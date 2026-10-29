Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 804fb9e39527c7d8ad6b71682b2c9d408f11768a

# An instrumentation-configuration ARN resolves only in the region it was minted in, breaking the crate's own "region is not part of a resource's identity" rule that the SLO branch of the same resolver applies

_Filed: 2026-10-28T16:20:39Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `804fb9e39527c7d8ad6b71682b2c9d408f11768a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`tagged_resource_arn` is the one place this crate turns a tagging operation's `ResourceArn` into the key a tag set is stored under, and it has two branches that disagree about whether an ARN's REGION field is part of a resource's identity.

* The SLO branch resolves through `tagged_slo_name_in_arn`, which deliberately IGNORES the region (it only narrows the vendor field and compares the account), so an SLO ARN carrying any region names the stored SLO.
* The instrumentation-configuration branch compares the request's ARN to each stored record's ARN as a WHOLE STRING (`configuration.arn == resource_arn`), which pins the region (and the partition) too. So an ARN that differs from the minted one only in its region segment names nothing, and `TagResource` (tag_resource.rs:186-194), `UntagResource` and `ListTagsForResource` answer 404 `ResourceNotFoundException` for a configuration this service is holding — tags cannot be attached to it, read back, or removed through that spelling of its ARN.

The region is not part of a configuration's identity by this crate's own account: `instrumentation_configuration_arn`'s doc names the identity an ARN spells as exactly four parts (service, environment, signal type, location hash), and the create/read path resolves a configuration by that identity alone — `tests/all/instrumentation_configuration_arn.rs` pins that a reader "in another region" gets the very same stored record. The stated reason for comparing whole ARNs is about the resource field ("a service or environment name may itself contain `/`"), which says nothing about the region, so the comparison pins a field the crate elsewhere says is only "a rendering of the writer's SigV4 credential scope".

The existing test pins the rule for the SLO kind only, and states it as holding for EVERY ARN this service resolves, so the configuration kind's divergence is both unpinned and contradicted by the crate's own text.

## Evidence

The two branches of one resolver — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:313-330:

    /// An instrumentation configuration is tagged too (`CreateInstrumentationConfiguration`
    /// takes a `TagList`), and its ARN carries the service and environment names, which may
    /// themselves contain `/` — so it is resolved by comparing against the ARN each stored
    /// record was minted with rather than by taking the identity apart again. The scan is
    /// bounded by [`MAX_INSTRUMENTATION_CONFIGURATIONS`].
    pub fn tagged_resource_arn(&self, resource_arn: &str) -> Option<&str> {
        if let Some(arn) = tagged_slo_name_in_arn(resource_arn)
            .and_then(|name| self.slos.get(name))
            .map(|slo| slo.arn.as_str())
        {
            return Some(arn);
        }
        self.instrumentation_configurations
            .values()
            .flat_map(|set| set.configurations.values())
            .find(|configuration| configuration.arn == resource_arn)
            .map(|configuration| configuration.arn.as_str())
    }

The rule the SLO branch applies — state.rs:616-624:

    /// The REGION field is not compared, for the reason the store's own doc gives: this process
    /// holds one account's and one Region's records, and the region in a minted ARN is a
    /// rendering of the writer's SigV4 scope rather than part of an SLO's identity.
    pub fn tagged_slo_name_in_arn(resource_arn: &str) -> Option<&str> {

The crate's own statement that a configuration's identity is four parts, none of them the region — state.rs:4018-4025:

    /// The ARN of the configuration at `location_hash` in `scope`, in the format the modelled
    /// `InstrumentationConfigurationArn` doc states:
    /// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
    ///
    /// The four parts it spells are a configuration's whole identity — the instrumentation type
    /// is not among them — which is the identity a create conflicts on

The handler that turns the `None` into a 404 — src/ops/tag_resource.rs:186-194:

        let Some(arn) = store
            .tagged_resource_arn(&input.resource_arn)
            .map(ToString::to_string)
        else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::tagged_resource_not_found(&input.resource_arn),
            )
            .into());
        };

Test coverage: the rule is pinned for the SLO kind ONLY, and the test states it as the rule for every ARN — tests/all/resource_tags.rs:548-567:

    /// An ARN of this service and account names the stored resource whatever REGION it carries:
    /// this process holds one account's and one Region's records, and the region a minted ARN
    /// carries is a rendering of the writer's SigV4 credential scope rather than part of the
    /// resource's identity (the rule `src/state.rs` states for every ARN this service resolves).
    #[tokio::test]
    async fn an_arn_resolves_whatever_region_it_carries() {
        ...
        for region in ["us-east-1", "ap-southeast-2"] {
            let arn = format!("arn:aws:application-signals:{region}:{ACCOUNT}:slo/{name}");
            let (status, body) = untag(&router, &arn, &[]).await;
            assert_eq!(status, 200, ...)

and the configuration's own reads are region-insensitive — tests/all/instrumentation_configuration_arn.rs:331-336:

        // The ARN is part of the STORED record, so a read reports the one the create minted —
        // even a read whose own credential scope names another region.
        for (who, reader) in [
            ("its own caller", case.caller),
            ("a caller in another region", Caller::Signed(ENDPOINT_REGION)),

No test in tests/all/instrumentation_configuration_tags.rs, resource_tags.rs or untag_resource_kinds.rs addresses a configuration by an ARN of another region, so nothing pins the current 404.

## Steps to Reproduce

1. Start the emulator with this crate mounted and sign the requests for us-east-1.
2. POST /create-instrumentation-configuration with {"Service":"checkout","Environment":"eks:prod","SignalType":"SNAPSHOT","InstrumentationType":"PROBE","Location":{"CodeLocation":{"Language":"Java","ClassName":"com.example.CartService","FilePath":"src/main/java/com/example/CartService.java","LineNumber":142}},"CaptureConfiguration":{"CodeCapture":{}}} and note the returned ARN: arn:aws:application-signals:us-east-1:<account>:instrumentationConfig/checkout/eks:prod/SNAPSHOT/<hash>.
3. POST /tag-resource with {"ResourceArn":"arn:aws:application-signals:eu-central-1:<account>:instrumentationConfig/checkout/eks:prod/SNAPSHOT/<hash>","Tags":[{"Key":"env","Value":"prod"}]} — the same ARN with only the region segment changed.
4. Observe HTTP 404 ResourceNotFoundException "InstrumentationConfiguration with arn '...' does not exist.", and the same 404 from GET /tags?ResourceArn=<that ARN> and POST /untag-resource with it, even though POST /get-instrumentation-configuration for the same identity still answers 200 from any signing region.
5. For contrast, POST /slo to create an SLO named "region-slo", then POST /tag-resource with {"ResourceArn":"arn:aws:application-signals:ap-southeast-2:<account>:slo/region-slo","Tags":[...]} and observe HTTP 200 — the SLO branch of the same resolver ignores the region, as tests/all/resource_tags.rs:553 pins.
6. Expected: the configuration's ARN names the one stored configuration whatever region it carries, since state.rs:4022 says the identity an ARN spells is the service, environment, signal type and location hash, and state.rs:616-618 says the region is a rendering of the writer's credential scope rather than part of a resource's identity.
