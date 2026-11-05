// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy a private CA carries: `PutPolicy` attaches it, `GetPolicy`
//! hands it back and `DeletePolicy` removes it, driven over the wire through the same
//! multi-service router the emulator boots. (`GetPolicy`'s own suite is
//! `resource_policy_read.rs`; this one is about the write and the delete, with the read as
//! the round trip's other half.)
//!
//! The rule under test: a policy operation resolves the resource-based policy of the
//! private CA its ARN names — attaching replaces what was attached, reading hands the
//! stored document back byte for byte, deleting removes it. A request naming no CA of this
//! service is refused with HTTP 400 `ResourceNotFoundException` about the CA; a resolved CA
//! carrying no policy is reported by the READ with the POLICY's own sentence, while the
//! DELETE succeeds — "If there is no policy attached to the private CA, this action will
//! return successful" (`DeletePolicy` docs). The documents below are ones this service
//! attaches: they grant actions a private CA is shared for, to identities it can resolve
//! (the tables for those two rules are `resource_policy_rules.rs`).

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    CA_NOT_FOUND_MESSAGE, INVALID_ARN_MESSAGE, INVALID_POLICY_MESSAGE, POLICY_NOT_FOUND_MESSAGE,
};

use crate::private_ca_lifecycle::{
    active_ca, blob, certificate_pem, root_ca_request, string, wait_for_the_clock, Emulator,
    ACCOUNT,
};

/// The three operations that read or write a CA's resource-based policy.
const POLICY_OPERATIONS: [&str; 3] = ["GetPolicy", "PutPolicy", "DeletePolicy"];

/// The `message` an error body carries, under either casing a protocol may use for it.
pub(crate) fn error_message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no error message in {body:?}"))
        .to_string()
}

/// A request body for `operation`, carrying whatever that operation needs: `PutPolicy`
/// also takes the document.
fn request(operation: &str, arn: &str, policy: &str) -> Value {
    if operation == "PutPolicy" {
        json!({"ResourceArn": arn, "Policy": policy})
    } else {
        json!({"ResourceArn": arn})
    }
}

/// Policy documents spanning what the member accepts, for the CA `ca_arn`: the case, the
/// document as it is SUBMITTED and the document as the CA then DISPLAYS it.
///
/// `PutPolicyRequest$Policy` is an `AWSPolicy`: "len 1..81920, pattern
/// `^[\u{0009}\u{000A}\u{000D}\u{0020}-\u{00FF}]+$`"
/// (`smithy_query(query="operation",operation="PutPolicy")`), i.e. JSON text of any size
/// in that range over that character class — so the rows differ in statement count, in
/// form (one statement object versus a list), in whitespace, in non-ASCII content up to
/// the pattern's last code point (U+00FF) and in size.
///
/// Every row is a policy OF `ca_arn`: a statement of a CA's resource policy names that CA
/// as its `Resource`, and the recorded refusal of a statement that names anything else
/// ("InvalidPolicy: The supplied policy contains different resource or incorrect ARN
/// format: <the CA>", replayed by
/// `issues/.../20261022T185258Z-acm-pca-policy-documents-are-never-reported-listpermissions.md`)
/// is what says so. A document a CA displays is the one submitted with the whitespace
/// between its tokens dropped, which is why the pretty-printed row's two columns differ.
fn policy_documents(ca_arn: &str) -> Vec<(&'static str, String, String)> {
    let one_statement = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "ShareWithAccount",
            "Effect": "Allow",
            "Principal": {"AWS": ACCOUNT},
            "Action": ["acm-pca:IssueCertificate", "acm-pca:GetCertificate"],
            "Resource": ca_arn
        }]
    });
    let statement_object = json!({
        "Version": "2012-10-17",
        "Statement": {
            "Effect": "Allow",
            "Principal": {"AWS": "*"},
            "Action": "acm-pca:DescribeCertificateAuthority",
            "Resource": ca_arn
        }
    });
    let three_statements = json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Principal": {"AWS": ACCOUNT},
             "Action": "acm-pca:IssueCertificate", "Resource": ca_arn},
            {"Effect": "Allow", "Principal": {"Service": "acm.amazonaws.com"},
             "Action": "acm-pca:GetCertificate", "Resource": ca_arn},
            {"Effect": "Deny", "Principal": "*",
             "Action": "acm-pca:ListTags", "Resource": ca_arn}
        ]
    });
    // U+00E9 and U+00FF are inside the pattern's `\u0020-\u00FF` range, so a document
    // carrying them is one the service accepts.
    let non_ascii = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "Pr\u{00E9}f\u{00E9}rence\u{00FF}",
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
            "Action": "acm-pca:IssueCertificate",
            "Resource": ca_arn
        }]
    });
    // Well inside the 81920-character ceiling, and far past anything a hand-written
    // fixture covers: size is one of the things the rule generalises over.
    let large = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "A".repeat(60_000),
            "Effect": "Allow",
            "Principal": {"AWS": ACCOUNT},
            "Action": "acm-pca:IssueCertificate",
            "Resource": ca_arn
        }]
    });
    let pretty_statement = json!({"Effect": "Allow", "Principal": {"AWS": ACCOUNT},
                                  "Action": "acm-pca:IssueCertificate", "Resource": ca_arn});

    vec![
        (
            "one statement",
            one_statement.to_string(),
            one_statement.to_string(),
        ),
        (
            "statement as an object",
            statement_object.to_string(),
            statement_object.to_string(),
        ),
        (
            "three statements",
            three_statements.to_string(),
            three_statements.to_string(),
        ),
        (
            "pretty printed with newlines and tabs",
            format!(
                "{{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{pretty_statement}\n\t]\n}}\r\n"
            ),
            // The members keep the order the document was written in; only the whitespace
            // between the tokens goes.
            format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{pretty_statement}]}}"),
        ),
        (
            "non-ASCII statement id",
            non_ascii.to_string(),
            non_ascii.to_string(),
        ),
        (
            "60 KB statement id",
            serde_json::to_string(&large).expect("the document serializes"),
            serde_json::to_string(&large).expect("the document serializes"),
        ),
    ]
}

/// The smallest policy of the CA `ca_arn`, for a case that only needs one.
fn policy_for(ca_arn: &str) -> String {
    json!({"Version": "2012-10-17",
           "Statement": [{"Effect": "Allow", "Principal": {"AWS": "*"},
                          "Action": "acm-pca:GetCertificate",
                          "Resource": ca_arn}]})
    .to_string()
}

/// Documents that are not resource-based policies: "If this policy does not contain all
/// required statements ... the PutPolicy action returns an InvalidPolicyException"
/// (`PutPolicyRequest$Policy` docs), and an IAM policy's statements are its `Statement`
/// element, each carrying an `Effect` of `Allow` or `Deny` and an `Action` — so these rows
/// span "not JSON at all", "JSON that is not a policy object", "a policy object with no
/// statement in it" and "a statement that is not one".
fn invalid_policy_documents() -> Vec<(&'static str, String)> {
    vec![
        ("not JSON", "this is not a policy".to_string()),
        ("a JSON string", "\"Allow\"".to_string()),
        (
            "a JSON array of policies",
            json!([{"Statement": [{"Effect": "Allow"}]}]).to_string(),
        ),
        (
            "no Statement element",
            json!({"Version": "2012-10-17"}).to_string(),
        ),
        (
            "an empty statement list",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
        ),
        (
            "an empty statement object",
            json!({"Version": "2012-10-17", "Statement": {}}).to_string(),
        ),
        (
            "a statement that is a string",
            json!({"Version": "2012-10-17", "Statement": "Allow"}).to_string(),
        ),
        (
            "a statement list of strings",
            json!({"Version": "2012-10-17", "Statement": ["Allow"]}).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Principal": {"AWS": "*"},
                                  "Action": "acm-pca:GetCertificate",
                                  "Resource": "*"}]})
            .to_string(),
        ),
        (
            "an Effect that is neither Allow nor Deny",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Effect": "Permit", "Principal": {"AWS": "*"},
                                  "Action": "acm-pca:GetCertificate",
                                  "Resource": "*"}]})
            .to_string(),
        ),
        (
            "a statement with no Action",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Effect": "Allow", "Principal": {"AWS": "*"},
                                  "Resource": "*"}]})
            .to_string(),
        ),
        (
            "one good statement beside one with no Action",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Effect": "Allow", "Principal": {"AWS": "*"},
                                  "Action": "acm-pca:GetCertificate", "Resource": "*"},
                                 {"Effect": "Deny", "Principal": {"AWS": "*"},
                                  "Resource": "*"}]})
            .to_string(),
        ),
    ]
}

/// Create a CA and leave it in PENDING_CERTIFICATE: created, with the clock's
/// CREATING -> PENDING_CERTIFICATE transition already due.
async fn pending_ca(emulator: &Emulator, common_name: &str) -> String {
    let created = emulator
        .ok(
            common_name,
            "CreateCertificateAuthority",
            &root_ca_request(common_name),
        )
        .await;
    let arn = string(&created, "CertificateAuthorityArn", common_name);
    wait_for_the_clock();
    arn
}

/// Every document the member accepts survives the attach / read / delete round trip
/// unchanged, and the delete is what makes the policy stop resolving.
#[tokio::test]
async fn a_policy_round_trips_through_put_get_and_delete() {
    let emulator = Emulator::new();
    let (arn, _certificate) = active_ca(&emulator, "policy-round-trip").await;

    for (case, document, displayed) in policy_documents(&arn) {
        emulator
            .ok(case, "PutPolicy", &request("PutPolicy", &arn, &document))
            .await;

        let read = emulator
            .ok(case, "GetPolicy", &request("GetPolicy", &arn, ""))
            .await;
        let returned = string(&read, "Policy", case);
        assert_eq!(
            returned, displayed,
            "case {case:?}: expected GetPolicy to hand back the attached document, got \
             {returned:?}"
        );

        emulator
            .ok(
                case,
                "DeletePolicy",
                &request("DeletePolicy", &arn, &document),
            )
            .await;

        // The deletion persisted: the policy resolves for nobody afterwards. The READ is
        // what reports the absence; the delete is delete-if-exists, so repeating it is a
        // success that leaves the CA carrying nothing.
        let body = emulator
            .refused(
                case,
                "GetPolicy",
                &request("GetPolicy", &arn, &document),
                "ResourceNotFoundException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, POLICY_NOT_FOUND_MESSAGE,
            "case {case:?}: expected GetPolicy after the delete to answer the service's \
             policy not-found sentence, got {message:?}"
        );
        emulator
            .ok(
                case,
                "DeletePolicy",
                &request("DeletePolicy", &arn, &document),
            )
            .await;

        // And the CA itself is untouched by its policy's deletion.
        let described = emulator
            .ok(
                case,
                "DescribeCertificateAuthority",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await;
        let status = string(
            described
                .get("CertificateAuthority")
                .unwrap_or(&Value::Null),
            "Status",
            case,
        );
        assert_eq!(
            status, "ACTIVE",
            "case {case:?}: expected the CA to stay ACTIVE across its policy's deletion, \
             got {status:?}"
        );
    }
}

/// A private CA this service resolves but that carries no policy: the READ reports the
/// policy as the absent resource, while the DELETE has nothing to remove and succeeds.
#[tokio::test]
async fn a_resolvable_ca_with_no_policy_resolves_no_policy() {
    let emulator = Emulator::new();
    let case = "ACTIVE CA with no policy";
    let (arn, _certificate) = active_ca(&emulator, "no-policy-active").await;

    let body = emulator
        .refused(
            case,
            "GetPolicy",
            &request("GetPolicy", &arn, ""),
            "ResourceNotFoundException",
        )
        .await;
    let message = error_message(&body, case);
    assert_eq!(
        message, POLICY_NOT_FOUND_MESSAGE,
        "case {case:?}: expected GetPolicy on a CA with no policy to answer the \
         service's policy not-found sentence, got {message:?}"
    );
    // The sentence the corpus recorded for this case, pinned as the literal it was
    // recorded as: the `GetPolicy` report
    // (issues/.../20261022T143159Z-acm-pca-getpolicy-stub.md) asserts
    // ("ResourceNotFoundException", "Could not find policy for specified resource.").
    assert_eq!(
        message, "Could not find policy for specified resource.",
        "case {case:?}: expected the recorded sentence from GetPolicy, got {message:?}"
    );

    // The delete of a policy that is not there is a success: "If there is no policy
    // attached to the private CA, this action will return successful" (`DeletePolicy`
    // docs).
    emulator
        .ok(case, "DeletePolicy", &request("DeletePolicy", &arn, ""))
        .await;

    // The same CA takes a policy, so the refusal above is about the policy and not
    // about the CA.
    let document = policy_for(&arn);
    emulator
        .ok(case, "PutPolicy", &request("PutPolicy", &arn, &document))
        .await;
}

/// An ARN that names no CA of this service resolves no policy either — for every policy
/// operation, including the one that would attach a document.
#[tokio::test]
async fn an_arn_naming_no_ca_resolves_no_policy() {
    let emulator = Emulator::new();
    // A CA exists, so the refusal is about the ARN in the request and not about an empty
    // store.
    let (existing, _certificate) = active_ca(&emulator, "unknown-ca-neighbour").await;
    // The CA is resolved before the document is judged, so the document the rows carry is
    // beside the point: what they pin is the refusal about the CA.
    let document = policy_for(&existing);

    // Both ARNs satisfy the modelled `Arn` pattern and name a certificate-authority of
    // this service that was never created: one in the CA's own account and region, one in
    // another account.
    let unknown = [
        (
            "an id no CA was minted with",
            format!("{existing}-does-not-exist"),
        ),
        (
            "another account's CA",
            "arn:aws:acm-pca:eu-west-1:999999999999:certificate-authority/\
             11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
    ];

    for (case, arn) in unknown {
        for operation in POLICY_OPERATIONS {
            let body = emulator
                .refused(
                    case,
                    operation,
                    &request(operation, &arn, &document),
                    "ResourceNotFoundException",
                )
                .await;
            let message = error_message(&body, case);
            // Every policy operation resolves the AUTHORITY before the policy, so all
            // three answer about the CA here: the read's own recorded case is
            // `acm-pca.ca-arn-validation-precedes-resource-lookup`, and the write and the
            // delete name the CA they were asked to attach to or to clear.
            assert_eq!(
                message, CA_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {operation} to answer the not-found sentence of \
                 the resource it was asked about, got {message:?}"
            );
        }
    }
}

/// An ARN that is well formed for the member but names no resource of this service is an
/// ARN error, not a not-found: the policy operations read the ARN the way every other
/// operation of the service does.
#[tokio::test]
async fn an_arn_of_another_resource_is_an_arn_error() {
    let emulator = Emulator::new();
    // The ARN is read before anything else, so the document the rows carry is beside the
    // point: what they pin is the refusal about the ARN.
    let document = policy_for("arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority/x");

    // Each row matches the modelled pattern
    // `^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]*:[\w+=,.@-]+(/[\w+=,.@-]+)*$`, so the
    // framework hands it to the handler, and none of them names a private CA.
    let rows = [
        (
            "no account segment",
            "arn:aws:acm-pca:eu-west-1::certificate-authority/\
             11111111-2222-3333-4444-555555555555",
        ),
        (
            "an account that is not twelve digits",
            "arn:aws:acm-pca:eu-west-1:12345:certificate-authority/\
             11111111-2222-3333-4444-555555555555",
        ),
        (
            "another resource type",
            "arn:aws:acm-pca:eu-west-1:000000000000:certificate/0123456789abcdef",
        ),
        (
            "no resource id",
            "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority",
        ),
    ];

    for (case, arn) in rows {
        for operation in POLICY_OPERATIONS {
            let body = emulator
                .refused(
                    case,
                    operation,
                    &request(operation, arn, &document),
                    "InvalidArnException",
                )
                .await;
            let message = error_message(&body, case);
            assert_eq!(
                message, INVALID_ARN_MESSAGE,
                "case {case:?}: expected {operation} to answer the service's ARN sentence, \
                 got {message:?}"
            );
        }
    }
}

/// `PutPolicy` refuses a document that is not a resource-based policy, and refusing it
/// leaves the CA's policy as it was.
#[tokio::test]
async fn put_policy_refuses_a_document_that_is_not_a_policy() {
    let emulator = Emulator::new();
    let (arn, _certificate) = active_ca(&emulator, "invalid-policy").await;
    let attached = policy_for(&arn);
    emulator
        .ok("attach", "PutPolicy", &request("PutPolicy", &arn, &attached))
        .await;

    for (case, document) in invalid_policy_documents() {
        let body = emulator
            .refused(
                case,
                "PutPolicy",
                &request("PutPolicy", &arn, &document),
                "InvalidPolicyException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, INVALID_POLICY_MESSAGE,
            "case {case:?}: expected PutPolicy to answer the service's policy sentence, got \
             {message:?}"
        );

        let read = emulator
            .ok(case, "GetPolicy", &request("GetPolicy", &arn, ""))
            .await;
        let returned = string(&read, "Policy", case);
        assert_eq!(
            returned, attached,
            "case {case:?}: expected the refused document to leave the attached policy \
             alone, got {returned:?}"
        );
    }
}

/// One policy per CA, and one CA per policy: attaching replaces what was attached, and
/// nothing a CA carries is visible through another CA's ARN.
#[tokio::test]
async fn a_policy_belongs_to_exactly_one_ca() {
    let emulator = Emulator::new();
    let (first, _first_certificate) = active_ca(&emulator, "policy-owner-one").await;
    let (second, _second_certificate) = active_ca(&emulator, "policy-owner-two").await;

    let documents = policy_documents(&first);
    let (_, first_document, _) = documents.first().expect("a document").clone();
    // The replacement is a policy of the FIRST CA too: a statement of a CA's policy names
    // that CA.
    let (_, first_replacement, first_replacement_displayed) =
        documents.get(2).expect("another document").clone();
    let (_, second_document, second_displayed) = policy_documents(&second)
        .get(2)
        .expect("another document")
        .clone();

    emulator
        .ok(
            "first",
            "PutPolicy",
            &request("PutPolicy", &first, &first_document),
        )
        .await;

    // The second CA resolves no policy while only the first carries one.
    emulator
        .refused(
            "second before its own policy",
            "GetPolicy",
            &request("GetPolicy", &second, ""),
            "ResourceNotFoundException",
        )
        .await;

    emulator
        .ok(
            "second",
            "PutPolicy",
            &request("PutPolicy", &second, &second_document),
        )
        .await;

    // Replacing the first CA's policy is an attach, not an append.
    emulator
        .ok(
            "first replaced",
            "PutPolicy",
            &request("PutPolicy", &first, &first_replacement),
        )
        .await;
    let read = emulator
        .ok("first replaced", "GetPolicy", &request("GetPolicy", &first, ""))
        .await;
    let returned = string(&read, "Policy", "first replaced");
    assert_eq!(
        returned, first_replacement_displayed,
        "case \"first replaced\": expected the newest document to replace the attached one, \
         got {returned:?}"
    );

    // Deleting one CA's policy leaves the other's where it was.
    emulator
        .ok(
            "delete first",
            "DeletePolicy",
            &request("DeletePolicy", &first, ""),
        )
        .await;
    let survivor = emulator
        .ok("delete first", "GetPolicy", &request("GetPolicy", &second, ""))
        .await;
    let survivor_document = string(&survivor, "Policy", "delete first");
    assert_eq!(
        survivor_document, second_displayed,
        "case \"delete first\": expected the other CA's policy to survive, got \
         {survivor_document:?}"
    );
}

/// A CA that does not hold its own certificate yet is still a CA this service resolves:
/// `DeletePolicy` succeeds against it whatever it carries, and `GetPolicy` reports the
/// policy it was asked for as the absent resource. Attaching a policy to such a CA is
/// accepted, and the delete removes it.
#[tokio::test]
async fn a_ca_without_its_certificate_resolves_for_the_policy_operations() {
    let emulator = Emulator::new();
    // `DeletePolicy` reads no document, so the rows below carry none.

    // A CA read straight after `CreateCertificateAuthority` is still CREATING; one read
    // after the transition the clock owes has come due is PENDING_CERTIFICATE. Neither
    // holds a certificate.
    let creating = emulator
        .ok(
            "CREATING CA",
            "CreateCertificateAuthority",
            &root_ca_request("policy-creating"),
        )
        .await;
    let creating = string(&creating, "CertificateAuthorityArn", "CREATING CA");
    let rows = [
        ("CREATING CA", creating),
        (
            "PENDING_CERTIFICATE CA",
            pending_ca(&emulator, "policy-pending").await,
        ),
    ];

    for (case, arn) in &rows {
        // Delete-if-exists: the CA resolves, so there is nothing left to report.
        emulator
            .ok(case, "DeletePolicy", &request("DeletePolicy", arn, ""))
            .await;

        // The read names the policy it was asked for, as its own recording has it.
        let body = emulator
            .refused(
                case,
                "GetPolicy",
                &request("GetPolicy", arn, ""),
                "ResourceNotFoundException",
            )
            .await;
        let message = error_message(&body, case);
        assert_eq!(
            message, POLICY_NOT_FOUND_MESSAGE,
            "case {case:?}: expected GetPolicy on a CA with no policy to answer the \
             service's policy not-found sentence, got {message:?}"
        );
    }

    // Attaching a policy to such a CA is accepted and the delete removes it.
    let case = "PENDING_CERTIFICATE CA with a policy attached";
    let (_, arn) = rows.last().expect("the pending row").clone();
    let attachable = policy_for(&arn);
    emulator
        .ok(case, "PutPolicy", &request("PutPolicy", &arn, &attachable))
        .await;
    let read = emulator
        .ok(case, "GetPolicy", &request("GetPolicy", &arn, ""))
        .await;
    let returned = string(&read, "Policy", case);
    assert_eq!(
        returned, attachable,
        "case {case:?}: expected the attached document back, got {returned:?}"
    );
    emulator
        .ok(case, "DeletePolicy", &request("DeletePolicy", &arn, ""))
        .await;
    let body = emulator
        .refused(
            case,
            "GetPolicy",
            &request("GetPolicy", &arn, ""),
            "ResourceNotFoundException",
        )
        .await;
    let message = error_message(&body, case);
    assert_eq!(
        message, POLICY_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the deleted policy to leave the CA carrying none, got \
         {message:?}"
    );

    // Importing the CA certificate changes none of it: the delete still succeeds and the
    // read still reports the policy as the absent resource.
    let case = "CA with its certificate imported";
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn,
                    "Certificate": blob(&certificate_pem("policy-pending"))}),
        )
        .await;
    emulator
        .ok(case, "DeletePolicy", &request("DeletePolicy", &arn, ""))
        .await;
    let body = emulator
        .refused(
            case,
            "GetPolicy",
            &request("GetPolicy", &arn, ""),
            "ResourceNotFoundException",
        )
        .await;
    let message = error_message(&body, case);
    assert_eq!(
        message, POLICY_NOT_FOUND_MESSAGE,
        "case {case:?}: expected GetPolicy to answer the service's policy not-found \
         sentence once the CA holds its certificate, got {message:?}"
    );
}
