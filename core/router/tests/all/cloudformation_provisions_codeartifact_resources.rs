// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CodeArtifact::Domain` and `AWS::CodeArtifact::Repository` resources its template
//! declares — in CodeArtifact, with the upstreams, the external connections and the
//! permissions policies the declarations carry.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns it created it, through that service's own public API and with every declared
//! property applied, so the handle the stack publishes addresses a resource that service
//! really holds — and deleting the stack releases it there.** For these two types the handle
//! a template reads is not the physical id: each type's `Ref` is the resource's ARN, and the
//! NAME every CodeArtifact call takes is one of the attributes `Fn::GetAtt` reads off it. So
//! right after the create waiter returns, `DescribeDomain`, `DescribeRepository` and
//! `GetRepositoryEndpoint` answer for the names the stack published, rather than
//! CodeArtifact answering `ResourceNotFoundException` for resources its own stack just
//! reported complete. Two crates have to agree for that to hold (CloudFormation makes the
//! calls, CodeArtifact serves them), so the suite lives here: a crate-local test can only
//! ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! domain and the repository or leaves both names to CloudFormation, whether it is written
//! in JSON or YAML, which optional properties it declares (description, encryption key,
//! upstreams, external connections, permissions policies, tags), how many resources the
//! stack holds, the LENGTH of the stack name a generated name is built from — the row that
//! names neither resource deploys under a stack name long enough that an unfitted
//! `<stack>-<logical id>-<unique id>` would break both of CodeArtifact's own name bounds, so
//! the create would be refused and the stack would roll back — and the Region it is deployed
//! in.
//!
//! The deletion half and the refusal half are rows of the same rule: deleting the stack
//! deletes what it created in CodeArtifact, and a declaration CodeArtifact REFUSES leaves the
//! stack `ROLLBACK_COMPLETE` naming the resource — with nothing half-created behind it, which
//! for a repository whose external connection is refused means no repository left behind
//! carrying none.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the deployment it completed. Neither CodeArtifact type has a
/// transient status of its own: a created domain is reported `Active` at once and a created
/// repository is usable at once.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries, and therefore the `owner` of every
/// domain and the `domainOwner` of every repository a stack creates.
const ACCOUNT: &str = "000000000000";

/// The longest domain name CodeArtifact accepts (`DomainName`, `@length(min: 2, max: 50)` in
/// that service's own model), which a GENERATED name is held to.
const MAX_DOMAIN_NAME: usize = 50;

/// The longest repository name it accepts (`RepositoryName`, `@length(min: 2, max: 100)`),
/// which a generated name is held to.
const MAX_REPOSITORY_NAME: usize = 100;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CodeArtifact operation over its restJson1 wire, signed for `region`: the method
/// and URI its own `@http` binding declares, with the resource it addresses in the query
/// string.
async fn codeartifact(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(document) => {
            Bytes::from(serde_json::to_vec(document).expect("the request body serializes"))
        }
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("codeartifact.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("codeartifact", region))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let mut document: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    // restJson1 names the error shape in a header; the suite reads it off the document so one
    // assertion can name the code whatever the envelope put it in.
    if let (Some(code), Some(object)) = (error_type, document.as_object_mut()) {
        object.insert("__errortype".to_string(), Value::String(code));
    }
    (status, document)
}

/// The `x-amzn-errortype` of a refused CodeArtifact answer, without the Coral namespace that
/// header may qualify it with.
fn error_code(payload: &Value) -> String {
    payload
        .get("__errortype")
        .and_then(Value::as_str)
        .map(|code| code.split(':').next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// The ARN a domain of `name` carries in `region`: the same total function of the Region, the
/// account and the name CodeArtifact itself keys a domain by.
fn domain_arn(region: &str, name: &str) -> String {
    format!("arn:aws:codeartifact:{region}:{ACCOUNT}:domain/{name}")
}

/// The ARN of a repository, which NESTS inside its domain's.
fn repository_arn(region: &str, domain: &str, repository: &str) -> String {
    format!("arn:aws:codeartifact:{region}:{ACCOUNT}:repository/{domain}/{repository}")
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// One output of the stack, which must be there: a key whose value never resolved is the
/// defect this suite is about.
fn output(outputs: &BTreeMap<String, String>, key: &str, case: &str) -> String {
    outputs
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no output {key:?} with a value in {outputs:?}"))
}

/// A domain's permissions policy, as a template writes one: an inline document, which is the
/// form `PermissionsPolicyDocument` publishes.
///
/// Each Aspen element the policy language types as "String or Array of strings" is written
/// here as the STRING, which is the form CodeArtifact reports a stored policy's in either
/// case — so this one document is both what the template declares and what the policy read
/// must answer with.
fn domain_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "codeartifact:GetAuthorizationToken",
            "Resource": "*",
        }],
    })
}

/// A repository's permissions policy, in the same form.
fn repository_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "codeartifact:DescribeRepository",
            "Resource": "*",
        }],
    })
}

/// The tags CodeArtifact holds on one resource, read through its own `ListTagsForResource`.
async fn tags_on(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, listed) = codeartifact(
        router,
        region,
        "POST",
        &format!("/v1/tags?resourceArn={resource_arn}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource on {resource_arn}, got \
         {status} with body {listed}"
    );
    listed
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|tag| {
            Some((
                tag.get("key").and_then(Value::as_str)?.to_string(),
                tag.get("value").and_then(Value::as_str)?.to_string(),
            ))
        })
        .collect()
}

/// One row of the table: a template, and what the rule says the domain and the repository it
/// declares must then be in CodeArtifact.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the resources are created in and
    /// the one their ARNs carry.
    region: &'static str,
    /// The stack the row deploys, whose NAME is the first segment of any name CloudFormation
    /// generates for a resource the template does not name.
    stack_name: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The domain name the template declares, or `None` for a template that names none —
    /// whose domain carries the name CloudFormation generated.
    declared_domain: Option<&'static str>,
    /// The repository name it declares, under the same rule.
    declared_repository: Option<&'static str>,
    /// The `description` `DescribeRepository` must report, if the row declares one.
    expected_description: Option<&'static str>,
    /// The `encryptionKey` `DescribeDomain` must report, if the row declares one.
    expected_encryption_key: Option<&'static str>,
    /// The external connections the repository must carry, as `(name, packageFormat)`.
    expected_connections: &'static [(&'static str, &'static str)],
    /// Whether the row declares an upstream repository for its own, which must come back as
    /// the upstream list the declaration asked for.
    expects_upstream: bool,
    /// The tags the repository's own declaration asks for.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The tags the DOMAIN's own declaration asks for.
    expected_domain_tags: &'static [(&'static str, &'static str)],
    /// Whether the row attaches a permissions policy to the domain and the repository.
    expects_policies: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named domain and repository in JSON, with an upstream, an external \
               connection, tags and a policy on each",
        region: "us-east-1",
        stack_name: "codeartifact-stack",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a CodeArtifact domain and a repository in it",
                "Resources": {
                    "Domain": {
                        "Type": "AWS::CodeArtifact::Domain",
                        "Properties": {
                            "DomainName": "orders-domain",
                            "PermissionsPolicyDocument": domain_policy(),
                            "Tags": [{ "Key": "Owner", "Value": "orders" }],
                        },
                    },
                    "Upstream": {
                        "Type": "AWS::CodeArtifact::Repository",
                        "Properties": {
                            "RepositoryName": "upstream-releases",
                            "DomainName": { "Fn::GetAtt": ["Domain", "Name"] },
                        },
                    },
                    "Repo": {
                        "Type": "AWS::CodeArtifact::Repository",
                        "Properties": {
                            "RepositoryName": "orders-releases",
                            "DomainName": { "Fn::GetAtt": ["Domain", "Name"] },
                            "Description": "the orders service's packages",
                            "Upstreams": [{ "Fn::GetAtt": ["Upstream", "Name"] }],
                            "ExternalConnections": ["public:npmjs"],
                            "PermissionsPolicyDocument": repository_policy(),
                            "Tags": [
                                { "Key": "Owner", "Value": "orders" },
                                { "Key": "Tier", "Value": "prod" },
                            ],
                        },
                    },
                },
                "Outputs": {
                    "DomainRef": { "Value": { "Ref": "Domain" } },
                    "DomainName": { "Value": { "Fn::GetAtt": ["Domain", "Name"] } },
                    "DomainArn": { "Value": { "Fn::GetAtt": ["Domain", "Arn"] } },
                    "DomainOwner": { "Value": { "Fn::GetAtt": ["Domain", "Owner"] } },
                    "RepoRef": { "Value": { "Ref": "Repo" } },
                    "RepoName": { "Value": { "Fn::GetAtt": ["Repo", "Name"] } },
                    "RepoArn": { "Value": { "Fn::GetAtt": ["Repo", "Arn"] } },
                    "RepoDomainOwner": { "Value": { "Fn::GetAtt": ["Repo", "DomainOwner"] } },
                },
            })
            .to_string()
        },
        declared_domain: Some("orders-domain"),
        declared_repository: Some("orders-releases"),
        expected_description: Some("the orders service's packages"),
        expected_encryption_key: None,
        expected_connections: &[("public:npmjs", "npm")],
        expects_upstream: true,
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
        expected_domain_tags: &[("Owner", "orders")],
        expects_policies: true,
    },
    Row {
        case: "a named domain and repository in YAML, with an encryption key and a different \
               external connection",
        region: "us-west-2",
        stack_name: "codeartifact-yaml-stack",
        template: || {
            [
                "Resources:",
                "  Domain:",
                "    Type: AWS::CodeArtifact::Domain",
                "    Properties:",
                "      DomainName: billing-domain",
                "      EncryptionKey: arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012",
                "  Repo:",
                "    Type: AWS::CodeArtifact::Repository",
                "    Properties:",
                "      RepositoryName: billing.releases_2026",
                "      DomainName: !GetAtt Domain.Name",
                "      ExternalConnections:",
                "        - public:pypi",
                "Outputs:",
                "  DomainRef:",
                "    Value: !Ref Domain",
                "  DomainName:",
                "    Value: !GetAtt Domain.Name",
                "  DomainArn:",
                "    Value: !GetAtt Domain.Arn",
                "  DomainOwner:",
                "    Value: !GetAtt Domain.Owner",
                "  RepoRef:",
                "    Value: !Ref Repo",
                "  RepoName:",
                "    Value: !GetAtt Repo.Name",
                "  RepoArn:",
                "    Value: !GetAtt Repo.Arn",
                "  RepoDomainOwner:",
                "    Value: !GetAtt Repo.DomainOwner",
                "",
            ]
            .join("\n")
        },
        declared_domain: Some("billing-domain"),
        declared_repository: Some("billing.releases_2026"),
        expected_description: None,
        expected_encryption_key: Some(
            "arn:aws:kms:us-west-2:000000000000:key/12345678-1234-1234-1234-123456789012",
        ),
        expected_connections: &[("public:pypi", "pypi")],
        expects_upstream: false,
        expected_tags: &[],
        expected_domain_tags: &[],
        expects_policies: false,
    },
    Row {
        case: "a domain and a repository the template names nowhere, under a stack name \
               longer than either name bound, with no optional property at all",
        region: "eu-west-1",
        // 96 characters, inside CloudFormation's own `StackName` rule and far longer than
        // CodeArtifact's 50-character domain bound and its 100-character repository bound,
        // so a name built from it has to be FITTED to each of them.
        stack_name: "codeartifact-generated-identity-suite-with-a-stack-name-long-enough-to-exceed-both-naming-bounds",
        template: || {
            json!({
                "Resources": {
                    "Domain": { "Type": "AWS::CodeArtifact::Domain", "Properties": {} },
                    "Repo": {
                        "Type": "AWS::CodeArtifact::Repository",
                        "Properties": {
                            "DomainName": { "Fn::GetAtt": ["Domain", "Name"] },
                        },
                    },
                },
                "Outputs": {
                    "DomainRef": { "Value": { "Ref": "Domain" } },
                    "DomainName": { "Value": { "Fn::GetAtt": ["Domain", "Name"] } },
                    "DomainArn": { "Value": { "Fn::GetAtt": ["Domain", "Arn"] } },
                    "DomainOwner": { "Value": { "Fn::GetAtt": ["Domain", "Owner"] } },
                    "RepoRef": { "Value": { "Ref": "Repo" } },
                    "RepoName": { "Value": { "Fn::GetAtt": ["Repo", "Name"] } },
                    "RepoArn": { "Value": { "Fn::GetAtt": ["Repo", "Arn"] } },
                    "RepoDomainOwner": { "Value": { "Fn::GetAtt": ["Repo", "DomainOwner"] } },
                },
            })
            .to_string()
        },
        declared_domain: None,
        declared_repository: None,
        expected_description: None,
        expected_encryption_key: None,
        expected_connections: &[],
        expects_upstream: false,
        expected_tags: &[],
        expected_domain_tags: &[],
        expects_policies: false,
    },
];

/// Every row: the handles the stack publishes for its CodeArtifact resources address a domain
/// and a repository CodeArtifact really holds, configured as the template declared them, and
/// deleting the stack releases both.
#[tokio::test]
async fn a_stack_provisions_the_codeartifact_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("{}-{index}", row.stack_name);
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Tags.member.1.Key", "Stack"),
                ("Tags.member.1.Value", "codeartifact-suite"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // Every output the template composes over the two resources carries a VALUE, and
        // each is the identifier that type publishes for it: the name `Fn::GetAtt … Name`
        // reads, the ARN `Ref` returns, and the account that owns the domain.
        let outputs = outputs_of(&described);
        let domain = output(&outputs, "DomainName", case);
        let repository = output(&outputs, "RepoName", case);
        if let Some(declared) = row.declared_domain {
            assert_eq!(
                domain, declared,
                "case {case:?}: expected the domain to carry the declared name {declared:?}, \
                 got {domain:?}"
            );
        }
        if let Some(declared) = row.declared_repository {
            assert_eq!(
                repository, declared,
                "case {case:?}: expected the repository to carry the declared name \
                 {declared:?}, got {repository:?}"
            );
        }
        assert!(
            domain.len() >= 2
                && domain.len() <= MAX_DOMAIN_NAME
                && domain.starts_with(|character: char| character.is_ascii_lowercase())
                && domain.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                }),
            "case {case:?}: expected a name inside CodeArtifact's own DomainName rule, got \
             {domain:?}"
        );
        assert!(
            repository.len() >= 2
                && repository.len() <= MAX_REPOSITORY_NAME
                && repository.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || character == '.'
                        || character == '_'
                        || character == '-'
                }),
            "case {case:?}: expected a name inside CodeArtifact's own RepositoryName rule, got \
             {repository:?}"
        );

        let expected_domain_arn = domain_arn(region, &domain);
        let expected_repository_arn = repository_arn(region, &domain, &repository);
        for (key, expected) in [
            ("DomainArn", &expected_domain_arn),
            ("DomainRef", &expected_domain_arn),
            ("RepoArn", &expected_repository_arn),
            ("RepoRef", &expected_repository_arn),
        ] {
            let reported = output(&outputs, key, case);
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected output {key:?} to be {expected}, got {reported}"
            );
        }
        for key in ["DomainOwner", "RepoDomainOwner"] {
            let reported = output(&outputs, key, case);
            assert_eq!(
                reported, ACCOUNT,
                "case {case:?}: expected output {key:?} to be the owning account {ACCOUNT}, got \
                 {reported}"
            );
        }

        // And CodeArtifact itself resolves those names to the resources the template
        // declared.
        let (status, answered) = codeartifact(
            &router,
            region,
            "GET",
            &format!("/v1/domain?domain={domain}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeDomain on the name the stack \
             reported, got {status} with body {answered}"
        );
        let reported = answered.get("domain").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported.get("name").and_then(Value::as_str),
            Some(domain.as_str()),
            "case {case:?}: expected the domain to report the name {domain:?}, got {reported}"
        );
        assert_eq!(
            reported.get("arn").and_then(Value::as_str),
            Some(expected_domain_arn.as_str()),
            "case {case:?}: expected the domain's ARN to be {expected_domain_arn}, got {reported}"
        );
        assert_eq!(
            reported.get("owner").and_then(Value::as_str),
            Some(ACCOUNT),
            "case {case:?}: expected the domain to be owned by {ACCOUNT}, got {reported}"
        );
        if let Some(key) = row.expected_encryption_key {
            assert_eq!(
                reported.get("encryptionKey").and_then(Value::as_str),
                Some(key),
                "case {case:?}: expected the declared encryption key, got {reported}"
            );
        }

        let addressed = format!("domain={domain}&repository={repository}");
        let (status, answered) = codeartifact(
            &router,
            region,
            "GET",
            &format!("/v1/repository?{addressed}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeRepository on the name the stack \
             reported, got {status} with body {answered}"
        );
        let reported = answered.get("repository").cloned().unwrap_or(Value::Null);
        assert_eq!(
            reported.get("name").and_then(Value::as_str),
            Some(repository.as_str()),
            "case {case:?}: expected the repository to report the name {repository:?}, got \
             {reported}"
        );
        assert_eq!(
            reported.get("arn").and_then(Value::as_str),
            Some(expected_repository_arn.as_str()),
            "case {case:?}: expected the repository's ARN to be {expected_repository_arn}, got \
             {reported}"
        );
        assert_eq!(
            reported.get("domainName").and_then(Value::as_str),
            Some(domain.as_str()),
            "case {case:?}: expected the repository to be in the declared domain {domain:?}, \
             got {reported}"
        );
        assert_eq!(
            reported.get("domainOwner").and_then(Value::as_str),
            Some(ACCOUNT),
            "case {case:?}: expected the repository's domain to be owned by {ACCOUNT}, got \
             {reported}"
        );
        assert_eq!(
            reported.get("description").and_then(Value::as_str),
            row.expected_description,
            "case {case:?}: expected the declared description, got {reported}"
        );
        let connections: Vec<(String, String)> = reported
            .get("externalConnections")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|connection| {
                Some((
                    connection
                        .get("externalConnectionName")
                        .and_then(Value::as_str)?
                        .to_string(),
                    connection.get("packageFormat").and_then(Value::as_str)?.to_string(),
                ))
            })
            .collect();
        let expected_connections: Vec<(String, String)> = row
            .expected_connections
            .iter()
            .map(|(name, format)| ((*name).to_string(), (*format).to_string()))
            .collect();
        assert_eq!(
            connections, expected_connections,
            "case {case:?}: expected the declared external connections \
             {expected_connections:?}, got {reported}"
        );
        let upstreams: Vec<String> = reported
            .get("upstreams")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|upstream| {
                Some(
                    upstream
                        .get("repositoryName")
                        .and_then(Value::as_str)?
                        .to_string(),
                )
            })
            .collect();
        let expected_upstreams: Vec<String> = if row.expects_upstream {
            vec!["upstream-releases".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            upstreams, expected_upstreams,
            "case {case:?}: expected the declared upstreams {expected_upstreams:?}, got \
             {reported}"
        );

        // The handle the stack published is usable: the repository endpoint a package
        // manager would be pointed at resolves for it.
        let (status, endpoint) = codeartifact(
            &router,
            region,
            "GET",
            &format!("/v1/repository/endpoint?{addressed}&format=npm"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRepositoryEndpoint, got {status} with \
             body {endpoint}"
        );
        assert!(
            endpoint
                .get("repositoryEndpoint")
                .and_then(Value::as_str)
                .is_some_and(|url| url.contains(&domain) && url.contains(&repository)),
            "case {case:?}: expected an endpoint naming the domain and the repository, got \
             {endpoint}"
        );

        // The policies the declarations attach are the ones CodeArtifact's own policy reads
        // report; a row that attaches none leaves each resource with no policy at all.
        for (uri, expected) in [
            (
                format!("/v1/domain/permissions/policy?domain={domain}"),
                domain_policy(),
            ),
            (
                format!("/v1/repository/permissions/policy?{addressed}"),
                repository_policy(),
            ),
        ] {
            let (status, answered) = codeartifact(&router, region, "GET", &uri, None).await;
            if !row.expects_policies {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected no policy on a resource the template attaches \
                     none to ({uri}), got {status} with body {answered}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the policy read ({uri}), got {status} \
                 with body {answered}"
            );
            let document = answered
                .pointer("/policy/document")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let parsed: Value = serde_json::from_str(document).unwrap_or(Value::Null);
            assert_eq!(
                parsed, expected,
                "case {case:?}: expected the declared policy document at {uri}, got {answered}"
            );
        }

        // The tags each declaration asked for are on its own resource, beside the stack's own
        // and the three `aws:cloudformation:*` ones CloudFormation records itself —
        // CodeArtifact's own tag writes take that namespace, so none is held back.
        for (logical_id, arn, declared) in [
            ("Domain", &expected_domain_arn, row.expected_domain_tags),
            ("Repo", &expected_repository_arn, row.expected_tags),
        ] {
            let tags = tags_on(&router, region, arn, case).await;
            for (key, value) in declared {
                assert_eq!(
                    tags.get(*key).map(String::as_str),
                    Some(*value),
                    "case {case:?}: expected tag {key:?} on {logical_id} to be {value:?}, got \
                     {tags:?}"
                );
            }
            assert_eq!(
                tags.get("Stack").map(String::as_str),
                Some("codeartifact-suite"),
                "case {case:?}: expected the stack's own tag on {logical_id}, got {tags:?}"
            );
            assert_eq!(
                tags.get("aws:cloudformation:logical-id").map(String::as_str),
                Some(logical_id),
                "case {case:?}: expected CloudFormation's own logical-id tag on {logical_id}, \
                 got {tags:?}"
            );
        }

        // Deleting the stack releases what CodeArtifact holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        for uri in [
            format!("/v1/repository?{addressed}"),
            format!("/v1/domain?domain={domain}"),
        ] {
            let (status, gone) = codeartifact(&router, region, "GET", &uri, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 from {uri} once the stack deleted it, got \
                 {status} with body {gone}"
            );
            assert_eq!(
                error_code(&gone),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException once the resource was \
                 deleted, got {gone}"
            );
        }
    }
}

/// A declaration CodeArtifact REFUSES does not report a resource nothing created: the stack
/// fails with that service's own refusal, and nothing half-created is left behind.
///
/// The rows differ along WHAT is refused and by WHICH call of the type's arm: a domain name a
/// client already holds (refused by the domain's create), a repository in a domain no domain
/// answers to (refused by the repository's create), and an external connection CodeArtifact
/// does not support (refused AFTER the repository was created, so the row also checks that
/// the repository that association was for is gone again).
#[tokio::test]
async fn a_declaration_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    // The domain a client already holds, which the first row's template re-declares.
    let (status, created) = codeartifact(
        &router,
        region,
        "POST",
        "/v1/domain?domain=already-there",
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own CreateDomain to succeed, got {status} with body {created}"
    );

    // (case, the stack's resources, the repository that must not be left behind)
    type RefusalRow<'a> = (&'a str, Value, Option<(&'a str, &'a str)>);
    let rows: Vec<RefusalRow<'_>> = vec![
        (
            "a domain name a client already holds",
            json!({
                "Domain": {
                    "Type": "AWS::CodeArtifact::Domain",
                    "Properties": { "DomainName": "already-there" },
                },
            }),
            None,
        ),
        (
            "a repository in a domain nothing holds",
            json!({
                "Repo": {
                    "Type": "AWS::CodeArtifact::Repository",
                    "Properties": {
                        "RepositoryName": "orphan-releases",
                        "DomainName": "no-such-domain",
                    },
                },
            }),
            None,
        ),
        (
            "an external connection CodeArtifact does not support",
            json!({
                "Domain": {
                    "Type": "AWS::CodeArtifact::Domain",
                    "Properties": { "DomainName": "connection-domain" },
                },
                "Repo": {
                    "Type": "AWS::CodeArtifact::Repository",
                    "Properties": {
                        "RepositoryName": "connection-releases",
                        "DomainName": { "Fn::GetAtt": ["Domain", "Name"] },
                        "ExternalConnections": ["public:does-not-exist"],
                    },
                },
            }),
            Some(("connection-domain", "connection-releases")),
        ),
    ];

    for (index, (case, resources, left_behind)) in rows.into_iter().enumerate() {
        let stack_name = format!("codeartifact-refusal-stack-{index}");
        let template = json!({ "Resources": resources }).to_string();
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back, got {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Domain") || reason.contains("Repo"),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        if let Some((domain, repository)) = left_behind {
            let (status, read) = codeartifact(
                &router,
                region,
                "GET",
                &format!("/v1/repository?domain={domain}&repository={repository}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected no repository to be left behind by the failed stack, \
                 got {status} with body {read}"
            );
        }
    }

    // The domain the CLIENT created is still exactly as it was: the stack's failure neither
    // replaced nor removed it.
    let (status, read) = codeartifact(
        &router,
        region,
        "GET",
        "/v1/domain?domain=already-there",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own domain to still resolve, got {status} with body {read}"
    );
}

/// A stack that declares no resource of either type still reports no CodeArtifact domain: the
/// arms are reached by the TYPE, not by anything in the request.
///
/// This is the control row of the rule above — a stack whose resource types this emulator
/// leaves declared-only must create nothing in CodeArtifact, and must still complete.
#[tokio::test]
async fn a_stack_declaring_no_codeartifact_resource_creates_none() {
    let router = build_router();
    let region = "ap-northeast-1";
    let template = json!({
        "Resources": {
            "Waiter": { "Type": "AWS::CloudFormation::WaitConditionHandle", "Properties": {} },
        },
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[
            ("StackName", "no-codeartifact-stack"),
            ("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, described) = cloudformation(
        &router,
        region,
        "DescribeStacks",
        &[("StackName", "no-codeartifact-stack")],
    )
    .await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the stack to complete, got {described}"
    );

    let (status, listed) =
        codeartifact(&router, region, "POST", "/v1/domains", Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListDomains, got {status} with body {listed}"
    );
    let domains = listed
        .get("domains")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        domains.is_empty(),
        "expected no domain to exist, got {listed}"
    );
}
