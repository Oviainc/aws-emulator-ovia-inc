// SPDX-License-Identifier: CC0-1.0

//! The form an attached IAM policy document comes back in, end to end over the wire.
//!
//! The rule under test: every ECR operation that echoes an attached policy document
//! reports it in the canonical IAM-policy rendering — pretty-printed two spaces per
//! enclosing object, a space on BOTH sides of the key separator, arrays inline, and a
//! value list holding exactly one value written as that value alone — while every token
//! of the caller's document survives untouched: its member order, its numbers, its string
//! escapes, and the brackets of a statement list that holds one statement.
//!
//! Each row below is one document and the exact bytes ECR reports for it, asserted
//! through the registry triple (`PutRegistryPolicy`, `GetRegistryPolicy`,
//! `DeleteRegistryPolicy`) and the repository triple (`SetRepositoryPolicy`,
//! `GetRepositoryPolicy`, `DeleteRepositoryPolicy`) — the six operations that report a
//! `policyText`. The rows differ along what the rule generalises over: the arity of a
//! value list (none, one, two), the element type of a list (strings against statement
//! objects), the nesting depth, the member order, the submitted whitespace, the character
//! set and the escapes.

use serde_json::{json, Value};

use crate::wire::{emulator, ok, REGISTRY_ID};

/// One row of the table: a document a client submits and the exact text ECR reports for
/// it.
pub struct Document {
    /// What the case is about, named in every assertion.
    pub case: &'static str,
    /// The document as the client writes it.
    pub submitted: &'static str,
    /// The document as ECR reports it back.
    pub reported: &'static str,
}

/// A document whose `Action` and `Resource` are written as ONE-VALUE LISTS: the IAM
/// policy language documents both as "a string or an array of strings", so a list of one
/// is reported as that string.
const ONE_VALUE_LISTS: Document = Document {
    case: "one-value Action and Resource lists",
    submitted: r#"{"Version": "2012-10-17", "Statement": [{"Sid": "AllowCrossAccountPull", "Effect": "Allow", "Principal": {"AWS": ["arn:aws:iam::123456789012:root"]}, "Action": ["ecr:ReplicateImage"], "Resource": ["arn:aws:ecr:eu-west-1:000000000000:repository/*"]}]}"#,
    reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "AllowCrossAccountPull",
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "arn:aws:iam::123456789012:root"
    },
    "Action" : "ecr:ReplicateImage",
    "Resource" : "arn:aws:ecr:eu-west-1:000000000000:repository/*"
  } ]
}"#,
};

/// The documents a client can submit, and the canonical rendering each comes back in.
pub fn documents() -> Vec<Document> {
    vec![
        ONE_VALUE_LISTS,
        Document {
            // The same document with its one-value lists written as the scalars they
            // mean: the two submissions are reported identically.
            case: "scalar Action and Resource",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowCrossAccountPull","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage","Resource":"arn:aws:ecr:eu-west-1:000000000000:repository/*"}]}"#,
            reported: ONE_VALUE_LISTS.reported,
        },
        Document {
            case: "a document already in the canonical rendering",
            submitted: ONE_VALUE_LISTS.reported,
            reported: ONE_VALUE_LISTS.reported,
        },
        Document {
            case: "a two-value Action list, which keeps its brackets",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowPull","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":["ecr:GetDownloadUrlForLayer","ecr:BatchGetImage"]}]}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "AllowPull",
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "arn:aws:iam::123456789012:root"
    },
    "Action" : [ "ecr:GetDownloadUrlForLayer", "ecr:BatchGetImage" ]
  } ]
}"#,
        },
        Document {
            // `Statement` holds STATEMENTS, not values, so a list of one statement is
            // not a one-value list and keeps its brackets — unlike the `Action` beside
            // it.
            case: "a statement written as a bare object rather than a list",
            submitted: r#"{"Statement":{"Effect":"Allow","Principal":{"Service":"replication.ecr.amazonaws.com"},"Action":"ecr:ReplicateImage"}}"#,
            reported: r#"{
  "Statement" : {
    "Effect" : "Allow",
    "Principal" : {
      "Service" : "replication.ecr.amazonaws.com"
    },
    "Action" : "ecr:ReplicateImage"
  }
}"#,
        },
        Document {
            case: "two statements, one with a two-value principal list",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root","arn:aws:iam::123456789012:role/replicator"]},"Action":"ecr:ReplicateImage"},{"Sid":"Two","Effect":"Deny","Principal":{"AWS":["arn:aws:iam::999988887777:root"]},"NotAction":["ecr:ReplicateImage"]}]}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "One",
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : [ "arn:aws:iam::123456789012:root", "arn:aws:iam::123456789012:role/replicator" ]
    },
    "Action" : "ecr:ReplicateImage"
  }, {
    "Sid" : "Two",
    "Effect" : "Deny",
    "Principal" : {
      "AWS" : "arn:aws:iam::999988887777:root"
    },
    "NotAction" : "ecr:ReplicateImage"
  } ]
}"#,
        },
        Document {
            case: "the member order the caller chose, under tabs and newlines",
            submitted: "{\n\t\"Statement\" : [\n\t\t{\n\t\t\t\"Action\" : \"ecr:ReplicateImage\" ,\n\t\t\t\"Effect\":\"Allow\",\n\t\t\t\"Principal\":{\"AWS\":\"arn:aws:iam::123456789012:root\"}\n\t\t}\n\t] ,\n\t\"Version\":\"2012-10-17\"\n}",
            reported: r#"{
  "Statement" : [ {
    "Action" : "ecr:ReplicateImage",
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "arn:aws:iam::123456789012:root"
    }
  } ],
  "Version" : "2012-10-17"
}"#,
        },
        Document {
            // The `Sid` carries characters from several scripts, one outside the basic
            // multilingual plane, an escaped non-ASCII character, an escaped quote and an
            // escaped backslash: a literal character is not escaped and an escape is not
            // decoded.
            case: "a document whose text is not ASCII, and whose escapes are its own",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Sid":"réplicação-日本語-🚀 \u00e9 \"quoted\" back\\slash","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"ecr:ReplicateImage"}]}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "réplicação-日本語-🚀 \u00e9 \"quoted\" back\\slash",
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "arn:aws:iam::123456789012:root"
    },
    "Action" : "ecr:ReplicateImage"
  } ]
}"#,
        },
        Document {
            // A condition's value lists are value lists too, and a condition's values
            // are not all strings: a number and a boolean keep the characters they were
            // written with. An empty list and an empty object are written `[ ]` and
            // `{ }`.
            case: "conditions, non-string values and empty elements",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["arn:aws:iam::123456789012:root"]},"Action":"ecr:ReplicateImage","Resource":[],"Condition":{"NumericLessThan":{"aws:MultiFactorAuthAge":3600},"Bool":{"aws:SecureTransport":true},"StringEquals":{"aws:PrincipalOrgID":["o-abc123"],"aws:RequestTag/team":["red","blue"]},"Null":{}}}]}"#,
            reported: r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "arn:aws:iam::123456789012:root"
    },
    "Action" : "ecr:ReplicateImage",
    "Resource" : [ ],
    "Condition" : {
      "NumericLessThan" : {
        "aws:MultiFactorAuthAge" : 3600
      },
      "Bool" : {
        "aws:SecureTransport" : true
      },
      "StringEquals" : {
        "aws:PrincipalOrgID" : "o-abc123",
        "aws:RequestTag/team" : [ "red", "blue" ]
      },
      "Null" : { }
    }
  } ]
}"#,
        },
    ]
}

/// The CONTENT of the policy document `text` holds, as a comparison can see it: the
/// parsed JSON with every one-value list of strings replaced by that string.
///
/// Two renderings of one document compare equal here whatever layout each is written in
/// and whichever of the two forms each writes a one-value list in, so a caller of this
/// says "the same policy" without saying anything about the rendering — the rendering is
/// pinned, byte for byte, by [`documents`].
pub fn document_content(text: &str) -> Value {
    fn content(value: &Value) -> Value {
        match value {
            Value::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), content(value)))
                    .collect(),
            ),
            Value::Array(entries) => match &entries[..] {
                [only @ Value::String(_)] => only.clone(),
                entries => Value::Array(entries.iter().map(content).collect()),
            },
            scalar => scalar.clone(),
        }
    }
    let parsed: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("the document must be JSON, got {error} in {text:?}"));
    content(&parsed)
}

/// The `policyText` a call reported.
fn reported_text(body: &Value) -> &str {
    body["policyText"]
        .as_str()
        .unwrap_or_else(|| panic!("the answer must carry a policyText, got {body:?}"))
}

/// The registry policy: the put reports the canonical rendering of the document, the read
/// reports the same text, re-putting that text changes nothing, and the delete reports
/// what it removed.
#[tokio::test]
async fn the_registry_policy_is_reported_in_canonical_form_by_all_three_operations() {
    let router = emulator();

    for document in documents() {
        let case = document.case;
        let put = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": document.submitted}),
        )
        .await;
        assert_eq!(
            reported_text(&put),
            document.reported,
            "case {case:?}: PutRegistryPolicy must report the canonical rendering, got {:?}",
            reported_text(&put)
        );
        assert_eq!(
            put["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the put names this registry, got {:?}",
            put["registryId"]
        );

        let got = ok(&router, case, "GetRegistryPolicy", &json!({})).await;
        assert_eq!(
            reported_text(&got),
            document.reported,
            "case {case:?}: GetRegistryPolicy must report the stored rendering, got {:?}",
            reported_text(&got)
        );

        // The rendering is a fixed point: a client that puts back what it read gets the
        // same text again, which is what makes a drift comparison settle.
        let again = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": document.reported}),
        )
        .await;
        assert_eq!(
            reported_text(&again),
            document.reported,
            "case {case:?}: putting the reported document back must change nothing, got {:?}",
            reported_text(&again)
        );

        let deleted = ok(&router, case, "DeleteRegistryPolicy", &json!({})).await;
        assert_eq!(
            reported_text(&deleted),
            document.reported,
            "case {case:?}: DeleteRegistryPolicy must report what it removed, got {:?}",
            reported_text(&deleted)
        );
    }
}

/// The repository policy: the same rendering, from the three operations that carry one.
#[tokio::test]
async fn the_repository_policy_is_reported_in_canonical_form_by_all_three_operations() {
    let router = emulator();

    for (index, document) in documents().into_iter().enumerate() {
        let case = document.case;
        let name = format!("canonical-policy-{index}");
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;

        let set = ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": document.submitted}),
        )
        .await;
        assert_eq!(
            reported_text(&set),
            document.reported,
            "case {case:?}: SetRepositoryPolicy must report the canonical rendering, got {:?}",
            reported_text(&set)
        );

        let got = ok(
            &router,
            case,
            "GetRepositoryPolicy",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            reported_text(&got),
            document.reported,
            "case {case:?}: GetRepositoryPolicy must report the stored rendering, got {:?}",
            reported_text(&got)
        );

        let again = ok(
            &router,
            case,
            "SetRepositoryPolicy",
            &json!({"repositoryName": name, "policyText": document.reported}),
        )
        .await;
        assert_eq!(
            reported_text(&again),
            document.reported,
            "case {case:?}: setting the reported document back must change nothing, got {:?}",
            reported_text(&again)
        );

        let deleted = ok(
            &router,
            case,
            "DeleteRepositoryPolicy",
            &json!({"repositoryName": name}),
        )
        .await;
        assert_eq!(
            reported_text(&deleted),
            document.reported,
            "case {case:?}: DeleteRepositoryPolicy must report what it removed, got {:?}",
            reported_text(&deleted)
        );
    }
}

/// The rendering depends on the document's CONTENT and nothing else: one policy submitted
/// in four layouts is reported as one text.
#[tokio::test]
async fn the_rendering_does_not_depend_on_the_submitted_layout() {
    let router = emulator();

    // The same document, written the ways a client's JSON writer writes one: with no
    // whitespace at all, with a space after each separator, indented four spaces, and
    // spread over lines with tabs.
    let indented_four_spaces = [
        "{\n",
        "    \"Version\": \"2012-10-17\",\n",
        "    \"Statement\": [\n",
        "        {\n",
        "            \"Sid\": \"Layout\",\n",
        "            \"Effect\": \"Allow\",\n",
        "            \"Principal\": {\n",
        "                \"AWS\": \"arn:aws:iam::123456789012:root\"\n",
        "            },\n",
        "            \"Action\": [\n",
        "                \"ecr:ReplicateImage\",\n",
        "                \"ecr:BatchImportUpstreamImage\"\n",
        "            ]\n",
        "        }\n",
        "    ]\n",
        "}",
    ]
    .concat();
    let tabs_and_line_breaks = [
        "{\t\"Version\"\t:\t\"2012-10-17\",\n",
        "\t\"Statement\"\t:\t[\t{\t\"Sid\"\t:\t\"Layout\",\n",
        "\"Effect\":\"Allow\",\"Principal\"\t:\t{\t\"AWS\"\t:\t\"arn:aws:iam::123456789012:root\"\t},\n",
        "\"Action\"\t:\t[\t\"ecr:ReplicateImage\",\t\"ecr:BatchImportUpstreamImage\"\t]\t}\t]\t}",
    ]
    .concat();
    let layouts = [
        (
            "no whitespace",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"Layout","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":["ecr:ReplicateImage","ecr:BatchImportUpstreamImage"]}]}"#
                .to_string(),
        ),
        (
            "a space after each separator",
            r#"{"Version": "2012-10-17", "Statement": [{"Sid": "Layout", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::123456789012:root"}, "Action": ["ecr:ReplicateImage", "ecr:BatchImportUpstreamImage"]}]}"#
                .to_string(),
        ),
        ("indented four spaces", indented_four_spaces),
        ("tabs and line breaks", tabs_and_line_breaks),
    ];

    let expected = r#"{
  "Version" : "2012-10-17",
  "Statement" : [ {
    "Sid" : "Layout",
    "Effect" : "Allow",
    "Principal" : {
      "AWS" : "arn:aws:iam::123456789012:root"
    },
    "Action" : [ "ecr:ReplicateImage", "ecr:BatchImportUpstreamImage" ]
  } ]
}"#;

    for (case, submitted) in layouts {
        let put = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": submitted}),
        )
        .await;
        assert_eq!(
            reported_text(&put),
            expected,
            "case {case:?}: every layout of one document is reported as one text, got {:?}",
            reported_text(&put)
        );
    }
}

/// Every key and every value of the submitted document survives the rendering, and the
/// rendering invents none: the reported text is the caller's own document, re-indented.
#[tokio::test]
async fn the_rendering_keeps_every_key_and_value_of_the_submitted_document() {
    let router = emulator();

    for document in documents() {
        let case = document.case;
        let put = ok(
            &router,
            case,
            "PutRegistryPolicy",
            &json!({"policyText": document.submitted}),
        )
        .await;
        let reported = reported_text(&put);
        assert_eq!(
            document_content(reported),
            document_content(document.submitted),
            "case {case:?}: the reported document must hold the submitted content, got {:?}",
            document_content(reported)
        );
    }
}
