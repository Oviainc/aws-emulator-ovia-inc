Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-instance-types-r9g-z1d
Issue-Step: task#19
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-instance-types-r9g-z1d ec2-catalog-describe-instance-types-c9gd-m6gd ec2-catalog-describe-vpc-endpoint-services
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].mediaAcceleratorInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.efaInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.efaSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].supportedBootModes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.24xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].mediaAcceleratorInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].supportedBootModes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.3xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].mediaAcceleratorInfo
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.encryptionInTransitSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].nitroEnclavesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].nitroTpmSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].placementGroupInfo.supportedStrategies.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].supportedBootModes.item
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="vt1.6xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.12xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.16xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.24xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.2xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.4xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].instanceStorageInfo.disks.item.count
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].instanceStorageInfo.disks.item.sizeInGB
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.8xlarge"].vCpuInfo.validCores.item[*]
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.ebsOptimizedInfo.baselineIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.large"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].cpuBurstModel
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].dedicatedHostsSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].ebsInfo.ebsOptimizedInfo.maximumIops
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].ebsInfo.ebsOptimizedInfo.maximumThroughputInMBps
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].ebsInfo.maximumEbsAttachments
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.defaultNetworkCardIndex
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.enaSrdSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.flexibleEnaQueuesSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.maximumNetworkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.networkCards
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.networkPerformance
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.secondaryNetworkSupported
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].networkInfo.trafficMirroringSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].phcSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].processorInfo.sustainedClockSpeedInGhz
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].rebootMigrationSupport
Issue-Key: ec2|DescribeInstanceTypes|2xx->2xx|$.DescribeInstanceTypesResponse.instanceTypeSet.item[instanceType="m5ad.xlarge"].vCpuInfo.validCores.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[*].baseEndpointDnsNameSet.item
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[*].privateDnsName
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item[*].privateDnsNameSet.item.privateDnsName
Issue-Related: 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md
Issue-Commit: 3b171590106c1f2d83787caf96ecc291950ccc13

# ec2 catalog pages r9g-z1d and vt1 plus the endpoint-service catalog serve synthesized rows

> _Note: this issue was identified against the code base at commit `3b171590106c1f2d83787caf96ecc291950ccc13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
This issue bundles TWO distinct causes, both in EC2's STATIC reference catalogs. BOTH must be closed before submitting.

--- Cause 1 of 2: DescribeInstanceTypes derives hardware facts from the instance-type size suffix instead of serving the per-type catalog, and omits modeled members ---
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): shape InstanceTypeInfo models MediaAcceleratorInfo (xmlName mediaAcceleratorInfo), NitroEnclavesSupport, NitroTpmSupport, PhcSupport, RebootMigrationSupport, DedicatedHostsSupported, SupportedBootModes, PlacementGroupInfo, VCpuInfo, EbsInfo, InstanceStorageInfo and NetworkInfo; shape NetworkInfo models DefaultNetworkCardIndex (defaultNetworkCardIndex), MaximumNetworkCards, NetworkCards, EfaSupported, EfaInfo, EnaSrdSupported, EncryptionInTransitSupported, FlexibleEnaQueuesSupport, SecondaryNetworkSupported and NetworkPerformance. (cpuBurstModel and networkInfo.trafficMirroringSupport appear in the recordings but are not in this model snapshot — recorded behavior only for those two.) Every one of these values is a FIXED per-instance-type fact from AWS's published catalog; it is not a function of the type's size suffix.
Emulator: whole families are answered from a size-derived template. The replay shows, for one page alone: mediaAcceleratorInfo, networkInfo.efaInfo and networkInfo.defaultNetworkCardIndex / maximumNetworkCards / networkCards absent; cpuBurstModel, phcSupport, rebootMigrationSupport, nitroEnclavesSupport, nitroTpmSupport, flexibleEnaQueuesSupport, secondaryNetworkSupported, encryptionInTransitSupported, enaSrdSupported and trafficMirroringSupport absent or wrong; ebsInfo.ebsOptimizedInfo baseline/maximum bandwidth-IOPS-throughput and ebsInfo.maximumEbsAttachments scaled wrongly (e.g. baselineBandwidthInMbps 20400 where AWS publishes 19000; maximumIops 90000 vs 80000; maximumEbsAttachments 28 vs 27 / 25); instanceStorageInfo.disks.item count/sizeInGB swapped for the same total (1x1800 GB vs AWS's 2x900 GB); processorInfo.sustainedClockSpeedInGhz wrong; dedicatedHostsSupported 'true' where AWS publishes 'false'; placementGroupInfo.supportedStrategies and supportedBootModes entries wrong; and vCpuInfo.validCores enumerating the wrong core counts for the type's vCPU count.
Why: this is the catalog every capacity-planning, cost and launch-eligibility decision reads. A client that picks an instance type by accelerator presence, EFA support, EBS throughput, local-disk layout or valid core counts gets a wrong answer and then fails (or silently over/under-provisions) at RunInstances; CPU-options requests built from validCores are rejected by the launch path.
Fix: the static instance-type catalog data and the DescribeInstanceTypes projection in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_types.rs — serve AWS's per-type rows (including the modeled members listed above) instead of synthesizing them from the size suffix.
Traces: ec2-catalog-describe-instance-types-r9g-z1d#task#19 (vt1 family) and ec2-catalog-describe-instance-types-c9gd-m6gd#task#37 (m5ad family). The same divergence_key (service=ec2|DescribeInstanceTypes|status=200->200) reproduces across every ec2-catalog-describe-instance-types-* page in the corpus — about two dozen net-new clusters in this run, all with stub_path and owned_by empty.

--- Cause 2 of 2: the full DescribeVpcEndpointServices catalog serves a different, shorter service list than AWS, built from one Interface-shaped template ---
AWS: Smithy shape ServiceDetail models serviceName, serviceId, serviceType, serviceRegion, availabilityZoneIdSet, availabilityZoneSet, owner, baseEndpointDnsNameSet, privateDnsName, privateDnsNameSet, vpcEndpointPolicySupported, acceptanceRequired, managesVpcEndpoints, payerResponsibility, tagSet, privateDnsNameVerificationState and supportedIpAddressTypeSet. Recorded behavior: the unfiltered catalog is AWS's own endpoint-service inventory for the region — a specific, ordered serviceNameSet with a matching serviceDetailSet whose per-service facts (acceptanceRequired, the four-AZ availabilityZoneSet/availabilityZoneIdSet including usw2-az4, the multi-entry privateDnsNameSet such as the api.aws alias, supportedIpAddressTypeSet of ipv4+ipv6, and the real vpce-svc-<17 hex> serviceId form) differ per service.
Emulator: the catalog contains different services in different positions (replay reports position-by-position mismatches across serviceNameSet.item[0..64] and serviceDetailSet.item[0..64]; e.g. where AWS's row 0 is com.amazonaws.us-west-2.access-analyzer the emulator serves com.amazonaws.us-west-2.appstream.api, and where AWS's row is com.amazonaws.us-west-2.kafka it serves com.amazonaws.us-west-2.kinesis-firehose), and every row is stamped from one template: three AZs (usw2-az1..3) instead of four, acceptanceRequired 'false' for services AWS marks 'true', a single-entry privateDnsNameSet, supportedIpAddressTypeSet of 'ipv4' only, a short 'vpce-svc-<8 hex>' serviceId, and an extra payerResponsibility member.
Why: a client that enumerates endpoint services to find the one it needs either does not find it or finds it with wrong facts — it then creates an endpoint in an AZ the service is not offered in, skips the connection-acceptance step AWS requires, or rejects dual-stack because ipv6 is not listed.
Fix: the endpoint-service catalog data and the DescribeVpcEndpointServices projection in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoint_services.rs — carry AWS's region inventory with per-service rows rather than one synthesized Interface template.
Traces: ec2-catalog-describe-vpc-endpoint-services#task#15, #task#17 and #task#24 (about ten net-new clusters on this fixture, all with stub_path and owned_by empty).

Related but NOT covering: open issues 20261108T143229Z-ec2-static-describe-catalogs-serve-synthesized-hardware-and.md, 20261109T072050Z-ec2-describeinstancetypes-derives-per-type-hardware-facts-from.md, 20261109T031226Z-ec2-catalog-pages-a1-c9g-and-c9gd-m6gd.md and the other open DescribeInstanceTypes catalog issues describe the same class of fault on OTHER catalog pages/families; the replay reports owned_by empty for every cluster cited above, so none of them carries these steps' replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 2742/3942 steps passed, 1081 mismatched, 602 divergence clusters.

DescribeInstanceTypes, ec2-catalog-describe-instance-types-r9g-z1d#task#19 (200 vs 200, stub_path empty, owned_by empty), vt1 family excerpt:
  cpuBurstModel: missing (expected 'none', got None)
  ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '19000', got '20400'
  ebsInfo.ebsOptimizedInfo.baselineIops: expected '80000', got '90000'
  ebsInfo.ebsOptimizedInfo.baselineThroughputInMBps: expected '2375.0', got '2550.0'
  ebsInfo.ebsOptimizedInfo.maximumBandwidthInMbps: expected '19000', got '20400'
  ebsInfo.maximumEbsAttachments: expected '27', got '28'
  mediaAcceleratorInfo: missing (expected {'accelerators': {'item': {'count': '8', 'manufacturer': 'Xilinx', 'memoryInfo': {'sizeInMiB': '24576'}, 'name': 'U30'}}, 'totalMediaMemoryInMiB': '196608'}, got None)
  networkInfo.defaultNetworkCardIndex: missing (expected '0', got None)
  networkInfo.efaInfo: missing (expected {'maximumEfaInterfaces': '1'}, got None)
  plus networkInfo.{efaSupported,enaSrdSupported,encryptionInTransitSupported,flexibleEnaQueuesSupport,maximumNetworkCards,networkCards,networkPerformance,secondaryNetworkSupported,trafficMirroringSupport}, nitroEnclavesSupport, nitroTpmSupport, phcSupport, placementGroupInfo.supportedStrategies[0..2], processorInfo.sustainedClockSpeedInGhz, rebootMigrationSupport, supportedBootModes and vCpuInfo.validCores[0..47] — for vt1.24xlarge, vt1.3xlarge and vt1.6xlarge.

DescribeInstanceTypes, ec2-catalog-describe-instance-types-c9gd-m6gd#task#37 (200 vs 200), m5ad family excerpt:
  dedicatedHostsSupported: expected 'false', got 'true'
  ebsInfo.ebsOptimizedInfo.baselineBandwidthInMbps: expected '6780', got '10200'
  ebsInfo.ebsOptimizedInfo.baselineIops: expected '30000', got '45000'
  ebsInfo.maximumEbsAttachments: expected '25', got '28'
  instanceStorageInfo.disks.item.count: expected '2', got '1'; disks.item.sizeInGB: expected '900', got '1800'
  networkInfo.defaultNetworkCardIndex / maximumNetworkCards / networkCards: missing
  vCpuInfo.validCores[0..23] wrong — for m5ad.2xlarge/4xlarge/8xlarge/12xlarge/16xlarge/24xlarge.

DescribeVpcEndpointServices, ec2-catalog-describe-vpc-endpoint-services#task#15, #task#17, #task#24 (200 vs 200, stub_path empty, owned_by empty):
  serviceNameSet.item[0..64] and serviceDetailSet.item[0..64] reported missing/mismatched position by position. Examples: expected row {serviceName 'com.amazonaws.us-west-2.access-analyzer', serviceId 'vpce-svc-0df7f8ad818c92d25', availabilityZoneIdSet ['usw2-az2','usw2-az1','usw2-az3','usw2-az4'], privateDnsNameSet [{'access-analyzer.us-west-2.amazonaws.com'},{'access-analyzer.us-west-2.api.aws'}], supportedIpAddressTypeSet ['ipv4','ipv6']}; got {serviceName 'com.amazonaws.us-west-2.appstream.api', serviceId 'vpce-svc-5fb66910', availabilityZoneIdSet ['usw2-az1','usw2-az2','usw2-az3'], single-entry privateDnsNameSet, supportedIpAddressTypeSet ['ipv4'], extra payerResponsibility 'ServiceOwner'}. Likewise expected 'com.amazonaws.us-west-2.kafka' (acceptanceRequired 'true') vs got 'com.amazonaws.us-west-2.kinesis-firehose' (acceptanceRequired 'false').

Model evidence: smithy_query shape=InstanceTypeInfo -> members MediaAcceleratorInfo, NitroEnclavesSupport, NitroTpmSupport, PhcSupport, RebootMigrationSupport, DedicatedHostsSupported, SupportedBootModes, PlacementGroupInfo, VCpuInfo, EbsInfo, InstanceStorageInfo, NetworkInfo. smithy_query shape=NetworkInfo -> DefaultNetworkCardIndex, MaximumNetworkCards, NetworkCards, EfaSupported, EfaInfo, EnaSrdSupported, EncryptionInTransitSupported, FlexibleEnaQueuesSupport, SecondaryNetworkSupported. smithy_query shape=ServiceDetail -> the 17 members listed above, supportedIpAddressTypeSet length 0..2.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay ec2-catalog-describe-instance-types-r9g-z1d step task#19 — DescribeInstanceTypes for the vt1 family. Observe mediaAcceleratorInfo / networkInfo.efaInfo / networkInfo.defaultNetworkCardIndex absent and ebsInfo, instanceStorageInfo, processorInfo and vCpuInfo.validCores values derived from the size suffix. Repeat at ec2-catalog-describe-instance-types-c9gd-m6gd step task#37 for the m5ad family (dedicatedHostsSupported true vs false, disks 1x1800 vs 2x900).
4. Cause 2: replay ec2-catalog-describe-vpc-endpoint-services steps task#15, task#17 and task#24 — unfiltered/paged DescribeVpcEndpointServices. Observe a different service list in different positions and every serviceDetailSet row stamped from one Interface template (three AZs, acceptanceRequired false, single privateDnsName, ipv4-only, short vpce-svc id, extra payerResponsibility).
