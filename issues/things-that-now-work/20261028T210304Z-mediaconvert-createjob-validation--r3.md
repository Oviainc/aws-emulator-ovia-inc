Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f6d3b7b16cb7e1ce86179d792dd5264644759e0e

# Queue/JobTemplate/JobId ARN references are taken apart by a second, namespace-blind ARN reader, so an ARN of another service resolves to a MediaConvert resource name

_Filed: 2026-10-28T21:03:04Z_
_Operation: CreateJob | Class: validation_

> _Note: this issue was identified against the code base at commit `f6d3b7b16cb7e1ce86179d792dd5264644759e0e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every resource reference this service resolves — `CreateJob$Queue`, `CreateJob$JobTemplate`, `CreateJobTemplate$Queue`, `UpdateJobTemplate$Queue`, `HopDestination$Queue`, `CreateResourceShare$JobId`, the `queue` filter of `ListJobs`/`SearchJobs` and the jobs-query `queue` filter key — goes through `crate::state::resource_name_from_reference`, which recognises an ARN by nothing more than `reference.rsplit_once(":<type>/")` plus `prefix.starts_with("arn:")`. It never checks that the ARN's service namespace is `mediaconvert`, and it never checks the field count.

So `arn:aws:s3:::queues/Default` is reduced to the resource name `Default`, which the store holds, and `CreateJob` with `Queue: "arn:aws:s3:::queues/Default"` is ACCEPTED: a job is stored on the built-in `Default` queue and projected back with `queue: "arn:aws:mediaconvert:<region>:<account>:queues/Default"` — a queue ARN the caller never sent, for a service the caller never named. The same value handed to any tagging door is refused with `BadRequestException` (400), and `tests/all/resource_tagging.rs` pins exactly that case under the label "an ARN of another service".

The state module's own ARN decomposer declares itself the single ARN reader, so this is the crate disagreeing with itself about what an ARN is: there are three readers (`arn_namespace_and_resource`, used by `parse_resource_arn` and `is_acm_certificate_arn`, and the hand-rolled one in `resource_name_from_reference`), and only the hand-rolled one accepts an ARN of another service. The same hole lets `arn:x:queues/Default` (a value with far too few ARN fields) resolve, and `arn:aws:mediaconvert:...:presets/p` resolve as a QUEUE when handed to `CreateJob$Queue`, because no resource-kind agreement is required beyond the literal `:queues/` substring the reader searched for.

## Evidence

The namespace-blind reader — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:962-974`:
```rust
/// The resource name inside a reference that is either a bare name or the ARN of a
/// MediaConvert resource of `resource_type`.
///
/// One function for every kind of reference this service resolves, because the resource id
/// of a MediaConvert ARN is `<plural type>/<name>` for all of them (see
/// [`TaggedResource::resource_id`]); a value that is not an ARN is the name itself.
fn resource_name_from_reference<'a>(resource_type: &str, reference: &'a str) -> &'a str {
    let separator = format!(":{resource_type}/");
    match reference.rsplit_once(separator.as_str()) {
        Some((prefix, name)) if prefix.starts_with("arn:") => name,
        _ => reference,
    }
}
```

The module's claim that there is only ONE such reader — `src/state.rs:1579-1598`:
```rust
/// The service namespace and the resource id of an ARN, or `None` when the value is not
/// an ARN at all.
///
/// `arn:<partition>:<service>:<region>:<account>:<resource id>` — the ONE place this
/// module takes an ARN apart, so the two kinds of ARN its request paths accept (this
/// service's own resources, and the ACM certificate the certificate operations name)
/// cannot disagree about what an ARN is.
fn arn_namespace_and_resource(arn: &str) -> Option<(&str, &str)> {
```

The reader that DOES check the namespace — `src/state.rs:1610-1614`:
```rust
pub fn parse_resource_arn(arn: &str) -> Option<TaggedResource> {
    let (namespace, resource_id) = arn_namespace_and_resource(arn)?;
    if namespace != ARN_NAMESPACE {
        return None;
    }
```

The accepting side — `src/ops/create_job.rs:347-359`:
```rust
            let queue = queue_reference
                .as_deref()
                .map(crate::state::queue_name_from_reference)
                .map(str::to_string)
                .unwrap_or_else(|| crate::state::DEFAULT_QUEUE_NAME.to_string());
            if !store.queues.contains_key(&queue) {
                return Err(CreateJobErrorModeled::NotFoundException(NotFoundException {
                    // The refusal names the request member the reference came from,
                    // `Queue`.
                    message: Some(crate::state::not_found_message("queue")),
                })
                .into());
            }
```
(`queue_name_from_reference` is `resource_name_from_reference(RESOURCE_TYPE_QUEUES, ..)` — `src/state.rs:938-940`.)

Test coverage: `tests/all/resource_tagging.rs:964-983` pins the 400 for the tagging doors —
```rust
/// A value that is not a MediaConvert resource ARN is a problem in the REQUEST: every
/// tagging operation answers the modelled `BadRequestException` (400), not a 404 and not
/// a success.
...
        ("an ARN of another service", "arn:aws:s3:::my-bucket".to_string()),
```
No test in the suite hands a cross-service ARN to a queue/job-template/job reference: grepping `tests/` for `:queues/`, `:jobTemplates/` and `:jobs/` finds only `tests/all/certificate_association.rs:286` (`arn:aws:mediaconvert:...:queues/Default`, used as a NON-certificate ARN), so no existing assertion changes.

## Steps to Reproduce

1. POST /2017-08-29/queues with {"name":"q1"} (optional; the built-in Default queue is enough).
2. POST /2017-08-29/jobs with {"role":"arn:aws:iam::000000000000:role/r","queue":"arn:aws:s3:::queues/Default","settings":{"inputs":[{"fileInput":"s3://b/k.mp4"}],"outputGroups":[{"outputs":[{"containerSettings":{"container":"MP4"}}]}]}}.
3. Observe HTTP 201: the job is created and its `queue` is reported as arn:aws:mediaconvert:<region>:<account>:queues/Default — an S3 ARN was accepted as a MediaConvert queue reference.
4. For comparison, POST /2017-08-29/tags with {"arn":"arn:aws:s3:us-west-2:000000000000:queues/Default","tags":{"a":"b"}} — HTTP 400 BadRequestException "Arn ... is not the ARN of a MediaConvert queue, preset, job or job template."
5. Also try POST /2017-08-29/resourceShares with {"jobId":"arn:aws:s3:::jobs/<the id from step 2>","supportCaseId":"c"} — HTTP 202, the share is recorded against the job.
