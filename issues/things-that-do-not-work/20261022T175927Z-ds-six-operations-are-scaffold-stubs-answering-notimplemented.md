Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.ip-route-nonexistent-directory-rejected ds.remove-region-access-denied ds.tag-ops-on-absent-directory-rejected ds.get-snapshot-limits-nonexistent-directory ds.enum-wire-values-precede-resource-resolution ds.describe-client-authentication-settings-nonexistent-directory
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.ip-route-nonexistent-directory-rejected|run#5|AssertionError|assert add_code == "EntityDoesNotExistException", (
Issue-Key: ds.remove-region-access-denied|run#18|AssertionError|assert denied_code == "AccessDeniedException", (
Issue-Key: ds.tag-ops-on-absent-directory-rejected|run#7|AssertionError|assert remove == "EntityDoesNotExistException", (
Issue-Key: ds.get-snapshot-limits-nonexistent-directory|run#8|AssertionError|assert code == "EntityDoesNotExistException", (
Issue-Key: ds.enum-wire-values-precede-resource-resolution|run#10|AssertionError|assert good_ldaps == "DirectoryDoesNotExistException", (
Issue-Key: ds.describe-client-authentication-settings-nonexistent-directory|run#7|AssertionError|assert code == "DirectoryDoesNotExistException", (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# ds: six operations are scaffold stubs answering NotImplemented instead of their modeled results/errors (6 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddIpRoutes, RemoveRegion, RemoveTagsFromResource, GetSnapshotLimits, DescribeLDAPSSettings, DescribeClientAuthenticationSettings

## Description
Six Directory Service operations answer every request with the scaffold response `NotImplemented` ("operation <X> is scaffolded but not implemented in this emulator") instead of the result or the modeled error the request calls for. Each is a separate handler and a separate cause; six causes in total, all in the directory-service crate. None of these six is covered by an existing issue (the auto-filed stub issues from this run name CreateMicrosoftAD, GetDirectoryLimits, DescribeSnapshots, DescribeEventTopics, StartADAssessment and ec2 CreateVpc - these six are different operations).

Cause 1 of 6: AddIpRoutes is a stub.
AWS: Smithy: AddIpRoutes input DirectoryId @pattern ^d-[0-9a-f]{10}$; modeled errors include EntityDoesNotExistException (client, HTTP 400). A well-formed but never-created directory id must be refused with EntityDoesNotExistException.
Emulator: returns error code NotImplemented.
Why: a client that adds an IP route for a directory cannot distinguish "no such directory" from a service fault, and no route can ever be added; retry/absence logic built on EntityDoesNotExistException never triggers.
Fix: the AddIpRoutes handler in the directory-service crate (ops/add_ip_routes.rs) plus its directory-resolution step.
Traces: ds.ip-route-nonexistent-directory-rejected ("expected EntityDoesNotExistException from AddIpRoutes for a nonexistent DirectoryId, got 'NotImplemented'").

Cause 2 of 6: RemoveRegion is a stub.
AWS: Smithy: RemoveRegion errors AccessDeniedException, ClientException, DirectoryDoesNotExistException, DirectoryUnavailableException, UnsupportedOperationException (all client, HTTP 400). An IAM-denied caller must get AccessDeniedException; an unrestricted caller against an absent directory must get DirectoryDoesNotExistException.
Emulator: returns NotImplemented for both callers.
Why: the IAM boundary around ds:RemoveRegion cannot be observed at all, and multi-region directory teardown is impossible.
Fix: the RemoveRegion handler (ops/remove_region.rs) and its authorization/resolution path.
Traces: ds.remove-region-access-denied ("expected AccessDeniedException for the IAM-denied caller, got 'NotImplemented'").

Cause 3 of 6: RemoveTagsFromResource is a stub.
AWS: Smithy: RemoveTagsFromResource input ResourceId @pattern ^[d]-[0-9a-f]{10}$; modeled errors ClientException, EntityDoesNotExistException, InvalidParameterException (client, HTTP 400). Removing tags from an absent directory is refused with EntityDoesNotExistException.
Emulator: returns NotImplemented.
Why: tag removal never takes effect, and a caller cannot tell a missing resource from a broken service; a NotImplemented is also not retriable or classifiable by SDK error handling.
Fix: the RemoveTagsFromResource handler (ops/remove_tags_from_resource.rs) and the tag store it should mutate.
Traces: ds.tag-ops-on-absent-directory-rejected ("RemoveTagsFromResource on absent d-74acc7c03b must raise EntityDoesNotExistException, got NotImplemented").

Cause 4 of 6: GetSnapshotLimits is a stub.
AWS: Smithy: GetSnapshotLimits output SnapshotLimits; modeled errors ClientException, EntityDoesNotExistException (client, HTTP 400), ServiceException (500). An absent directory id yields EntityDoesNotExistException.
Emulator: returns NotImplemented.
Why: snapshot quota discovery is unavailable, and absence is indistinguishable from failure.
Fix: the GetSnapshotLimits handler (ops/get_snapshot_limits.rs) plus snapshot-limit state.
Traces: ds.get-snapshot-limits-nonexistent-directory ("GetSnapshotLimits(DirectoryId='d-1dca5812e8') must raise EntityDoesNotExistException, got 'NotImplemented'").

Cause 5 of 6: DescribeLDAPSSettings is a stub.
AWS: Smithy: DescribeLDAPSSettings input Type (enum LDAPSType), Limit @range 1-50; modeled errors ClientException, DirectoryDoesNotExistException, InvalidNextTokenException, InvalidParameterException, UnsupportedOperationException. A request with a valid enum wire value against an absent directory must clear input validation and fail resolution with DirectoryDoesNotExistException.
Emulator: returns NotImplemented, so neither the enum-validation leg nor the resolution leg can be observed.
Why: LDAPS state cannot be read, and the validate-then-resolve ordering that clients depend on is unobservable.
Fix: the DescribeLDAPSSettings handler (ops/describe_ldaps_settings.rs), its enum validation and the LDAPS settings state.
Traces: ds.enum-wire-values-precede-resource-resolution ("DescribeLDAPSSettings(Type='Client') on absent d-709343894e must ... fail resolution with DirectoryDoesNotExistException ..., got NotImplemented").

Cause 6 of 6: DescribeClientAuthenticationSettings is a stub.
AWS: Smithy: DescribeClientAuthenticationSettings errors AccessDeniedException, ClientException, DirectoryDoesNotExistException, InvalidParameterException, UnsupportedOperationException. A well-formed but never-created DirectoryId must be refused with DirectoryDoesNotExistException.
Emulator: returns NotImplemented.
Why: client-authentication settings cannot be read and the not-found family for this operation is wrong, so callers branching on DirectoryDoesNotExistException break.
Fix: the DescribeClientAuthenticationSettings handler (ops/describe_client_authentication_settings.rs) and its directory resolution.
Traces: ds.describe-client-authentication-settings-nonexistent-directory ("must raise DirectoryDoesNotExistException, got NotImplemented").

## Evidence
Replay of the behavioral suite against the emulator (ds 0/145 passed):
- ds.ip-route-nonexistent-directory-rejected: expected EntityDoesNotExistException from AddIpRoutes for a nonexistent DirectoryId, got 'NotImplemented'.
- ds.remove-region-access-denied: expected AccessDeniedException for the IAM-denied caller, got 'NotImplemented'.
- ds.tag-ops-on-absent-directory-rejected: RemoveTagsFromResource on absent d-74acc7c03b must raise EntityDoesNotExistException, got NotImplemented.
- ds.get-snapshot-limits-nonexistent-directory: GetSnapshotLimits(DirectoryId='d-1dca5812e8') must raise EntityDoesNotExistException, got 'NotImplemented'.
- ds.enum-wire-values-precede-resource-resolution: DescribeLDAPSSettings(Type='Client') on absent d-709343894e must get past validation and fail resolution with DirectoryDoesNotExistException, got NotImplemented.
- ds.describe-client-authentication-settings-nonexistent-directory: DescribeClientAuthenticationSettings(DirectoryId=d-813a518427) must raise DirectoryDoesNotExistException, got NotImplemented.
In every case the emulator body is the scaffold text "operation <X> is scaffolded but not implemented in this emulator", so the expected status/error-code pair (400 with the modeled client error) is never produced.

## Steps to Reproduce
1. Boot the emulator and point a botocore `ds` client at it.
2. ds.add_ip_routes(DirectoryId='d-1dca5812e8', IpRoutes=[{'CidrIp': '198.51.100.0/24'}]) -> expect EntityDoesNotExistException, observe NotImplemented.
3. ds.remove_region(DirectoryId='d-1dca5812e8') -> expect DirectoryDoesNotExistException (AccessDeniedException for an IAM-denied principal), observe NotImplemented.
4. ds.remove_tags_from_resource(ResourceId='d-74acc7c03b', TagKeys=['k']) -> expect EntityDoesNotExistException, observe NotImplemented.
5. ds.get_snapshot_limits(DirectoryId='d-1dca5812e8') -> expect EntityDoesNotExistException, observe NotImplemented.
6. ds.describe_ldaps_settings(DirectoryId='d-709343894e', Type='Client') -> expect DirectoryDoesNotExistException, observe NotImplemented.
7. ds.describe_client_authentication_settings(DirectoryId='d-813a518427', Type='SmartCard') -> expect DirectoryDoesNotExistException, observe NotImplemented.
