// SPDX-License-Identifier: CC0-1.0

//! The context keys a supplied set of IAM policy documents references, end to end
//! over the awsQuery wire.
//!
//! `GetContextKeysForCustomPolicy` reports the condition context keys referenced by
//! the policies the REQUEST carries — "the policies are supplied as a list of one or
//! more strings" — so nothing it answers depends on what the account holds. The
//! awsQuery harness (router, call, element, elements, error_code) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! condition operator, the number of keys in one operator block, the number of
//! operators and of statements, the form of `Statement`, the number of documents in
//! `PolicyInputList`, the service namespace a key is drawn from, and the case the
//! keys are spelled in.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// One `GetContextKeysForCustomPolicy` call carrying `supplied` as
/// `PolicyInputList`.
async fn context_keys(router: &MultiServiceRouter, supplied: &[&str]) -> (u16, String) {
    let names: Vec<String> = (1..=supplied.len())
        .map(|position| format!("PolicyInputList.member.{position}"))
        .collect();
    let mut params: Vec<(&str, &str)> = Vec::new();
    for (name, document) in names.iter().zip(supplied) {
        params.push((name.as_str(), document));
    }
    call(router, "GetContextKeysForCustomPolicy", &params).await
}

/// The reported key names, sorted, so an assertion is about the SET the operation
/// reports rather than the order it happens to list it in.
fn reported_keys(body: &str) -> Vec<String> {
    let mut keys = elements(body, "member");
    keys.sort();
    keys
}

/// The expected key names, sorted the same way.
fn expected_keys(keys: &[&str]) -> Vec<String> {
    let mut expected: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
    expected.sort();
    expected
}

/// One request: the documents it supplies and the keys the operation must report.
struct Case {
    case: &'static str,
    supplied: &'static [&'static str],
    expected: &'static [&'static str],
}

/// Rows spanning the condition forms the IAM policy grammar admits — one operator,
/// several keys in one operator block, several operators in one condition, several
/// statements, a bare statement object, an `IfExists` operator and a set operator
/// (`ForAllValues:`), a statement with no condition at all — and the shapes the
/// supplied list can take: one document, several documents, documents sharing a key,
/// and a document that references none.
///
/// Most key names are AWS global condition context keys (IAM User Guide, "AWS global
/// condition context keys"); two rows draw their keys from a SERVICE namespace
/// (`s3:prefix`, `dynamodb:LeadingKeys`), because the rule under test is which keys a
/// document REFERENCES, not which keys exist, so nothing here depends on a catalog.
const CASES: &[Case] = &[
    Case {
        case: "one operator testing one key",
        supplied: &[
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/team":"storage"}}}]}"#,
        ],
        expected: &["aws:PrincipalTag/team"],
    },
    Case {
        case: "two keys tested by one operator",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"DateGreaterThan":{"aws:CurrentTime":"2026-01-01T00:00:00Z","aws:TokenIssueTime":"2026-01-01T00:00:00Z"}}}]}"#,
        ],
        expected: &["aws:CurrentTime", "aws:TokenIssueTime"],
    },
    Case {
        case: "two operators in one condition",
        supplied: &[
            r#"{"Statement":[{"Effect":"Deny","Action":"iam:*","Condition":{"StringEquals":{"aws:username":"alice"},"Bool":{"aws:MultiFactorAuthPresent":"true"}}}]}"#,
        ],
        expected: &["aws:username", "aws:MultiFactorAuthPresent"],
    },
    Case {
        case: "two statements each carrying a condition",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}},{"Effect":"Deny","Action":"s3:*","Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}"#,
        ],
        expected: &["aws:SourceIp", "aws:SecureTransport"],
    },
    Case {
        case: "a bare statement object rather than a list",
        supplied: &[
            r#"{"Statement":{"Effect":"Allow","Action":"ec2:*","Condition":{"StringEquals":{"aws:RequestedRegion":"us-east-1"}}}}"#,
        ],
        expected: &["aws:RequestedRegion"],
    },
    Case {
        case: "an IfExists operator and a set operator",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"StringEqualsIfExists":{"aws:PrincipalOrgID":"o-abc"},"ForAllValues:StringEquals":{"aws:TagKeys":["team"]}}}]}"#,
        ],
        expected: &["aws:PrincipalOrgID", "aws:TagKeys"],
    },
    Case {
        case: "a key in a service namespace rather than the global one",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*","Condition":{"StringLike":{"s3:prefix":["home/"]}}}]}"#,
        ],
        expected: &["s3:prefix"],
    },
    Case {
        case: "a service key beside a global one",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"dynamodb:Query","Condition":{"ForAllValues:StringEquals":{"dynamodb:LeadingKeys":["alice"]},"Bool":{"aws:SecureTransport":"true"}}}]}"#,
        ],
        expected: &["dynamodb:LeadingKeys", "aws:SecureTransport"],
    },
    Case {
        case: "a policy that tests nothing",
        supplied: &[
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#,
        ],
        expected: &[],
    },
    Case {
        case: "a policy carrying no statement of its own",
        supplied: &[r#"{"Statement":[]}"#],
        expected: &[],
    },
    Case {
        case: "one key repeated across statements and operators",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"StringEquals":{"aws:PrincipalAccount":"000000000000"}}},{"Effect":"Deny","Action":"*","Condition":{"StringNotEquals":{"aws:PrincipalAccount":"000000000001"}}}]}"#,
        ],
        expected: &["aws:PrincipalAccount"],
    },
    Case {
        case: "three documents, one of them testing nothing",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"StringLike":{"aws:ResourceTag/project":"atlas*"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"kms:*"}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"sts:*","Condition":{"ArnEquals":{"aws:SourceArn":"arn:aws:sns:us-east-1:000000000000:topic"}}}]}"#,
        ],
        expected: &["aws:ResourceTag/project", "aws:SourceArn"],
    },
    Case {
        case: "two documents sharing one key",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#,
            r#"{"Statement":[{"Effect":"Deny","Action":"*","Condition":{"Bool":{"aws:SecureTransport":"false"},"StringEquals":{"aws:PrincipalType":"User"}}}]}"#,
        ],
        expected: &["aws:SecureTransport", "aws:PrincipalType"],
    },
    Case {
        case: "five documents, each testing one key",
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"Bool":{"aws:ViaAWSService":"true"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"NumericLessThan":{"aws:MultiFactorAuthAge":"3600"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"DateLessThan":{"aws:EpochTime":"2000000000"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"StringEquals":{"aws:PrincipalOrgPaths":"o-abc/r-root/"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"ArnLike":{"aws:PrincipalArn":"arn:aws:iam::000000000000:role/*"}}}]}"#,
        ],
        expected: &[
            "aws:ViaAWSService",
            "aws:MultiFactorAuthAge",
            "aws:EpochTime",
            "aws:PrincipalOrgPaths",
            "aws:PrincipalArn",
        ],
    },
];

/// Every key the supplied documents test is reported, each key once, and nothing
/// else is.
#[tokio::test]
async fn every_key_the_supplied_policies_test_is_reported() {
    let router = router();
    for case in CASES {
        let Case {
            case,
            supplied,
            expected,
        } = case;
        let (status, body) = context_keys(&router, supplied).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read must succeed, got {status}: {body}"
        );
        assert_eq!(
            reported_keys(&body),
            expected_keys(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// The keys come from the REQUEST, so the same documents answer the same keys
/// whatever the account holds, and the call leaves the account as it was.
#[tokio::test]
async fn the_report_depends_on_the_request_rather_than_the_account() {
    let document = r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"StringEquals":{"aws:PrincipalTag/team":"storage"}}}]}"#;

    // An empty account first: nothing has been created, so a key reported here can
    // only have come from the supplied document.
    let router = router();
    let (status, empty_store) = context_keys(&router, &[document]).await;
    assert_eq!(
        status, 200,
        "the read must succeed on an empty account, got {status}: {empty_store}"
    );
    assert_eq!(
        reported_keys(&empty_store),
        expected_keys(&["aws:PrincipalTag/team"]),
        "expected the supplied document's key, got {empty_store}"
    );

    // Then an account carrying a user whose inline policy tests a DIFFERENT key:
    // that key belongs to the principal's policies, which this operation never
    // reads ("if you want to include only a list of policies by string, use
    // GetContextKeysForCustomPolicy instead").
    let (status, body) = call(&router, "CreateUser", &[("UserName", "CustomKeys")]).await;
    assert_eq!(
        status, 200,
        "CreateUser must succeed, got {status}: {body}"
    );
    let arn = element(&body, "Arn");
    let (status, body) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "CustomKeys"),
            ("PolicyName", "Embedded"),
            (
                "PolicyDocument",
                r#"{"Statement":[{"Effect":"Allow","Action":"ec2:*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "PutUserPolicy must succeed, got {status}: {body}"
    );

    let (status, populated_store) = context_keys(&router, &[document]).await;
    assert_eq!(
        status, 200,
        "the read must succeed, got {status}: {populated_store}"
    );
    assert_eq!(
        reported_keys(&populated_store),
        reported_keys(&empty_store),
        "the account's own policies must not change the report, got {populated_store}"
    );

    // A read reports: the user and its inline policy are untouched, and the
    // principal-policy reader still sees them.
    let (status, read_back) = call(
        &router,
        "GetUserPolicy",
        &[("UserName", "CustomKeys"), ("PolicyName", "Embedded")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the inline policy must survive the read, got {status}: {read_back}"
    );
    assert_eq!(
        element(&read_back, "PolicyName"),
        "Embedded",
        "the read must leave the inline policy as it was, got {read_back}"
    );

    // The two readers share ONE definition of "the keys these documents
    // reference": the principal reader, given the same document beside a principal
    // carrying one key, reports that key as well as its own.
    let mut params: Vec<(&str, &str)> = vec![("PolicySourceArn", &arn)];
    params.push(("PolicyInputList.member.1", document));
    let (status, principal) = call(&router, "GetContextKeysForPrincipalPolicy", &params).await;
    assert_eq!(
        status, 200,
        "the principal read must succeed, got {status}: {principal}"
    );
    assert_eq!(
        reported_keys(&principal),
        expected_keys(&["aws:PrincipalTag/team", "aws:SecureTransport"]),
        "expected the supplied and the embedded key, got {principal}"
    );
}

/// Context key names are not distinguished by case, so two spellings of one key are
/// ONE reported key.
#[tokio::test]
async fn keys_are_folded_without_regard_to_case() {
    // Rows: the spellings a document can carry for one key — as AWS documents it,
    // lower case, upper case and inverted case — each in its own supplied document.
    let spellings = [
        "aws:username",
        "AWS:USERNAME",
        "Aws:UserName",
        "aWS:uSERNAME",
    ];

    let router = router();
    let documents: Vec<String> = spellings
        .iter()
        .map(|spelling| {
            format!(
                r#"{{"Statement":[{{"Effect":"Allow","Action":"s3:*","Condition":{{"StringEquals":{{"{spelling}":"alice"}}}}}}]}}"#
            )
        })
        .collect();
    let supplied: Vec<&str> = documents.iter().map(String::as_str).collect();

    let (status, body) = context_keys(&router, &supplied).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    let reported = reported_keys(&body);
    assert_eq!(
        reported.len(),
        1,
        "the four spellings name one key, so one key must be reported, got {body}"
    );
    assert_eq!(
        reported[0].to_ascii_lowercase(),
        "aws:username",
        "expected the key the documents test, got {body}"
    );

    // Each spelling alone is still that one key, so the fold is about the name and
    // not about the order the list happens to carry.
    for (position, document) in supplied.iter().enumerate() {
        let (status, body) = context_keys(&router, &[document]).await;
        assert_eq!(
            status, 200,
            "spelling {position}: the read must succeed, got {status}: {body}"
        );
        let reported = reported_keys(&body);
        assert_eq!(
            reported.len(),
            1,
            "spelling {position}: expected one key, got {body}"
        );
        assert_eq!(
            reported[0].to_ascii_lowercase(),
            "aws:username",
            "spelling {position}: expected the key the document tests, got {body}"
        );
    }
}

/// A supplied member that is not an IAM policy document is refused as invalid
/// input, and the refusal is about the MEMBER rather than the list.
#[tokio::test]
async fn a_supplied_policy_that_is_not_a_document_is_refused() {
    let router = router();

    // The same table of non-documents the principal reader's suite refuses, because
    // the rule is the one shared definition of an IAM policy document.
    let malformed = [
        ("not JSON at all", "not a policy"),
        ("a bare string", r#""Statement""#),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#),
        ("a JSON number", "17"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        ("an empty object", "{}"),
        ("a truncated object", r#"{"Statement":[{"Effect":"Allow"}"#),
        ("a lower-case statement key", r#"{"statement":[]}"#),
    ];
    let valid = r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#;
    for (case, document) in malformed {
        // Alone, first in the list, and last in it, so the refusal is about the
        // member wherever the list carries it.
        for (position, supplied) in [
            vec![document],
            vec![document, valid],
            vec![valid, document],
        ]
        .into_iter()
        .enumerate()
        {
            let (status, body) = context_keys(&router, &supplied).await;
            assert_eq!(
                status, 400,
                "case {case:?} at position {position}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?} at position {position}: expected the modelled awsQuery code, \
                 got {body}"
            );
        }
    }

    // The list is required and carries "a list of one or more strings", so a
    // request naming no policy at all is refused rather than answered with an empty
    // key set — by the boundary validator, before the handler runs, because
    // `PolicyInputList` is a required member.
    let (status, body) = context_keys(&router, &[]).await;
    assert_eq!(
        status, 400,
        "case \"no member at all\": expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "case \"no member at all\": expected the boundary refusal of a missing required \
         member under the one input-refusal code, got {body}"
    );

    // A refused request reports nothing and changes nothing: a list of documents
    // that ARE policies still reads.
    let (status, body) = context_keys(&router, &[valid]).await;
    assert_eq!(
        status, 200,
        "the read must still succeed after the refusals, got {status}: {body}"
    );
    assert_eq!(
        reported_keys(&body),
        expected_keys(&["aws:SecureTransport"]),
        "expected the supplied document's key, got {body}"
    );
}
