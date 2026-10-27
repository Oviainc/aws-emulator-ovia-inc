// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate's test target drives the emulator through.
//!
//! One copy, shared by the suites declared in `tests/all.rs`: a request is built as the
//! form-encoded body an AWS SDK sends, dispatched through a router carrying only this
//! service, and answered with the HTTP status and the XML body — so what a suite asserts
//! is what a client sees.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    ComAmazonawsCloudformationCloudformationState, STATUS_TRANSITION_DELAY,
};
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the tests' requests are signed for.
pub const REGION: &str = "us-east-1";

/// A router carrying only this service.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The router AND the engine behind it, for a case that has to inspect the store or seed
/// it with more records than a bound's worth of wire calls could create.
///
/// The services that OWN the resource types `state::provisioning` creates are mounted
/// beside CloudFormation on the SAME engine, exactly as `core/router/src/lib.rs` mounts
/// them: a stack's create calls those services, so a suite that deploys a template
/// declaring one of those types must have them there — Amazon SNS among them, since an
/// `AWS::SNS::Topic` a template declares is really created in SNS by the stack. (Amazon
/// Data Firehose is deliberately NOT one of them — `unprovisioned_resources.rs` is the
/// suite that pins what a stack does when an owning service cannot be reached.)
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    // Amazon RDS, for the `AWS::RDS::DBInstance` and `AWS::RDS::DBSubnetGroup` a stack
    // really creates there: a suite that deploys one reads the database back through RDS's
    // own wire (`stack_lifecycle::rds`).
    router.register_service(service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(
        Arc::clone(&engine),
    ));
    // AWS Systems Manager, for the `AWS::SSM::Parameter` a stack really creates there.
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// This service's store in the shared engine, as every handler reaches it.
pub fn store(
    engine: &Arc<engine::InMemoryEngine>,
) -> engine::ServiceState<ComAmazonawsCloudformationCloudformationState> {
    engine.state::<ComAmazonawsCloudformationCloudformationState>()
}

/// Wait past the instant a stack operation's terminal status becomes due.
pub async fn wait_for_the_transition() {
    tokio::time::sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// Call one operation: `Action` plus the request's members, form-encoded exactly as an AWS
/// SDK sends them, answered with the HTTP status and the XML body.
///
/// The body is built with `form_urlencoded`, the encoder behind the `url` crate, rather
/// than a percent-encoder of this suite's own.
pub async fn call(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    call_in(router, REGION, action, members).await
}

/// [`call`], signed for `region`: CloudFormation is Regional, so the credential scope of
/// the request is what decides which Region's stacks it reads and writes.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut form = form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", action);
    form.append_pair("Version", "2010-05-15");
    for (key, value) in members {
        form.append_pair(key, value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cloudformation.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cloudformation/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    (status, body)
}

/// One request member as the pair [`call`] takes.
pub fn member(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Undo the XML text escaping the serializer applies, so a round-tripped value can be
/// compared with what was sent.
pub fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Every `<tag>` text in document order.
pub fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(xml_unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    found
}

/// The first `<tag>` text, or `None` when the response carries none.
pub fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// `CreateStack` with a template body and whatever else the case needs.
pub async fn create_stack(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("StackName", name), member("TemplateBody", template)];
    members.extend_from_slice(extra);
    call(router, "CreateStack", &members).await
}

/// The inner text of every non-nested `<tag …>…</tag>` BLOCK, in document order, left
/// exactly as the serializer wrote it.
///
/// [`xml_texts`] is the leaf form of this scan: it unescapes what it finds, which is right
/// for a value and wrong for a block that carries more elements. A caller reading a nested
/// list (the `<member>`s inside one `<Details>`) takes the block first and scans INSIDE it,
/// so a sibling list's members cannot be swept up with it.
///
/// The open tag may carry ATTRIBUTES — the query serializer puts an `xmlns` on the first
/// member of a list it renders — so the element NAME is matched and whatever follows it up
/// to the `>` is skipped. A longer element name that merely starts with `tag` is not that
/// element (`<Tagging>` is not a `<Tag>`), so the scan moves past it.
pub fn xml_blocks(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
            rest = &rest[start + 1..];
            continue;
        }
        let Some(open_end) = after.find('>') else { break };
        let inner = &after[open_end + 1..];
        let Some(end) = inner.find(&close) else { break };
        found.push(inner[..end].to_string());
        rest = &inner[end + close.len()..];
    }
    found
}

/// One `ListImports` read: the HTTP status, the XML body, the stack names the page reports
/// in wire order, and the `NextToken` that resumes the read just past them.
///
/// The ONE `ListImports` reader of this crate's suites — `cross_stack_imports.rs` asserts
/// WHICH stacks the import index holds and `list_imports_pages.rs` how that index is paged —
/// so neither can drift about how the response is read. The status and the body come back
/// rather than being asserted here, because a case that sends a cursor the service never
/// issued is asserting the REFUSAL.
pub async fn list_imports_page(
    router: &MultiServiceRouter,
    region: &str,
    export_name: &str,
    token: Option<&str>,
) -> (u16, String, Vec<String>, Option<String>) {
    let mut members = vec![member("ExportName", export_name)];
    if let Some(token) = token {
        members.push(member("NextToken", token));
    }
    let (status_code, body) = call_in(router, region, "ListImports", &members).await;
    // The `Imports` list is read INSIDE its own block, so a `<member>` of another list the
    // envelope carries cannot be swept up with it.
    let names = match xml_blocks(&body, "Imports").into_iter().next() {
        Some(block) => xml_texts(&block, "member"),
        None => Vec::new(),
    };
    let next_token = xml_text(&body, "NextToken");
    (status_code, body, names, next_token)
}

/// [`list_imports_page`] for a case that expects the read to SUCCEED: the page's stack names
/// and its `NextToken`.
///
/// `ListImports` models no error, so every well-formed read of it is a 200 — including a read
/// of an export name nothing publishes — and that one sentence is asserted here rather than
/// in each suite that reads the index.
pub async fn list_imports_ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    export_name: &str,
    token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let (status_code, body, names, next_token) =
        list_imports_page(router, region, export_name, token).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected ListImports to succeed, got {status_code} {body}"
    );
    (names, next_token)
}

/// `GetTemplate` at one stage, or at the service's default when `stage` is `None`.
pub async fn get_template(
    router: &MultiServiceRouter,
    name_or_id: &str,
    stage: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", name_or_id)];
    if let Some(stage) = stage {
        members.push(member("TemplateStage", stage));
    }
    call(router, "GetTemplate", &members).await
}
