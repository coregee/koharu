mod graph;
mod loader;
mod packages;

use std::{collections::HashMap, fmt::Display, hash::Hash, path::PathBuf};

use anyhow::{Context, Result};

use crate::{Backend, Device, Hardware};
use graph::{Component, Plan};
use packages::{Diffusion, Llama};

pub use packages::Torch;

mod sealed {
    pub trait Sealed {}
}

/// An immutable package managed by Koharu's runtime store.
#[allow(async_fn_in_trait)]
pub trait Package: sealed::Sealed + Copy + Display + Send + Sync + 'static {
    async fn install(self) -> Result<PathBuf>;
}

pub(crate) trait RuntimePackage: Package + std::fmt::Debug + Eq + Hash {
    const NAME: &'static str;

    fn dependencies(self, _hardware: &Hardware) -> Result<Vec<Component>> {
        Ok(Vec::new())
    }

    async fn activate(self) -> Result<()>;

    fn label(self) -> String {
        format!("{} {self}", Self::NAME)
    }
}

pub(crate) trait DiscoverablePackage: RuntimePackage {
    fn discover(hardware: &Hardware) -> Option<Self>;
}

/// A process-wide runtime capability requested by a consumer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Feature {
    Torch,
    Llama,
    Diffusion,
}

/// A discovered, dependency-ordered runtime plan.
#[derive(Debug)]
pub struct Runtime {
    candidates: Vec<RuntimeCandidate>,
}

#[derive(Debug)]
struct RuntimeCandidate {
    plan: Plan,
    hardware: Hardware,
    devices: HashMap<Feature, Backend>,
}

/// Devices selected for the initialized runtime and each requested feature.
#[derive(Clone, Debug)]
pub struct RuntimeDevices {
    primary: Device,
    devices: HashMap<Feature, Device>,
}

impl RuntimeDevices {
    /// Returns the primary hardware candidate used to build the runtime plan.
    #[must_use]
    pub fn primary(&self) -> &Device {
        &self.primary
    }

    /// Returns the device selected for a requested runtime feature.
    #[must_use]
    pub fn device(&self, feature: Feature) -> Option<&Device> {
        self.devices.get(&feature)
    }
}

impl Runtime {
    pub fn discover(features: impl IntoIterator<Item = Feature>) -> Result<Self> {
        let features = features.into_iter().collect::<Vec<_>>();
        let hardware = Hardware::discover();
        let mut candidates = Vec::new();
        for candidate in hardware.candidates() {
            if let Some((plan, devices)) = Self::plan(&features, &candidate)? {
                candidates.push(RuntimeCandidate {
                    plan,
                    hardware: candidate,
                    devices,
                });
            }
        }
        if candidates.is_empty() {
            anyhow::bail!("no device supports the requested runtime features")
        }
        Ok(Self { candidates })
    }

    fn plan(
        features: &[Feature],
        hardware: &Hardware,
    ) -> Result<Option<(Plan, HashMap<Feature, Backend>)>> {
        let mut plan = Plan::default();
        let mut devices = HashMap::new();
        let mut previous = None;

        for feature in features {
            let Some((node, backend)) = (match feature {
                Feature::Torch => plan
                    .require::<Torch>(hardware)?
                    .map(|(node, package)| (node, package.device_backend(hardware))),
                Feature::Llama => plan
                    .require::<Llama>(hardware)?
                    .map(|(node, package)| (node, package.backend())),
                Feature::Diffusion => plan
                    .require::<Diffusion>(hardware)?
                    .map(|(node, package)| (node, package.backend())),
            }) else {
                return Ok(None);
            };
            devices.insert(*feature, backend);
            if let Some(previous) = previous
                && previous != node
            {
                plan.sequence(previous, node);
            }
            previous = Some(node);
        }
        Ok(Some((plan, devices)))
    }

    /// Installs and activates packages, then returns the selected feature devices.
    #[tracing::instrument(skip_all)]
    pub async fn initialize(self) -> Result<RuntimeDevices> {
        let mut failures = Vec::new();
        for candidate in self.candidates {
            let rocm_index = match candidate.plan.preflight().await {
                Ok(index) => index,
                Err(error) => {
                    let device = candidate
                        .hardware
                        .device()
                        .map_or_else(|| "CPU".to_owned(), |device| device.description.clone());
                    tracing::warn!(
                        %device,
                        %error,
                        "runtime accelerator preflight failed; trying the next candidate"
                    );
                    failures.push(format!("{device}: {error:#}"));
                    continue;
                }
            };
            return candidate.initialize(rocm_index).await;
        }
        anyhow::bail!(
            "no runtime candidate passed accelerator preflight: {}",
            failures.join("; ")
        )
    }
}

impl RuntimeCandidate {
    async fn initialize(self, rocm_index: Option<usize>) -> Result<RuntimeDevices> {
        let primary = self.hardware.device().cloned().unwrap_or_else(Device::cpu);
        let primary = self.plan.initialize(primary, rocm_index).await?;
        let devices = self
            .devices
            .into_iter()
            .map(|(feature, backend)| {
                let device = if backend == primary.backend {
                    Some(primary.clone())
                } else if backend == Backend::Cpu {
                    Some(Device::cpu())
                } else {
                    self.hardware
                        .devices
                        .iter()
                        .find(|device| device.backend == backend)
                        .cloned()
                }
                .with_context(|| format!("no {backend} device was discovered for {feature:?}"))?;
                Ok((feature, device))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(RuntimeDevices { primary, devices })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    use crate::DeviceType;

    #[test]
    fn runtime_devices_expose_primary_and_feature_devices() {
        let primary = Device::rocm(3);
        let llama = Device::vulkan(0);
        let devices = RuntimeDevices {
            primary: primary.clone(),
            devices: HashMap::from([
                (Feature::Torch, primary.clone()),
                (Feature::Llama, llama.clone()),
            ]),
        };

        assert_eq!(devices.primary(), &primary);
        assert_eq!(devices.device(Feature::Torch), Some(&primary));
        assert_eq!(devices.device(Feature::Llama), Some(&llama));
        assert_eq!(devices.device(Feature::Diffusion), None);
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn gfx1151_routes_native_packages_to_vulkan_but_keeps_torch_on_rocm() {
        let hardware = rocm_hardware("gfx1151", true);
        let (_, devices) = Runtime::plan(
            &[Feature::Torch, Feature::Llama, Feature::Diffusion],
            &hardware,
        )
        .unwrap()
        .unwrap();

        assert_eq!(devices[&Feature::Torch], Backend::Rocm);
        assert_eq!(devices[&Feature::Llama], Backend::Vulkan);
        assert_eq!(devices[&Feature::Diffusion], Backend::Vulkan);
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn gfx1151_keeps_native_packages_on_rocm_without_vulkan() {
        let hardware = rocm_hardware("gfx1151", false);
        let (_, devices) = Runtime::plan(
            &[Feature::Torch, Feature::Llama, Feature::Diffusion],
            &hardware,
        )
        .unwrap()
        .unwrap();

        assert_eq!(devices[&Feature::Torch], Backend::Rocm);
        assert_eq!(devices[&Feature::Llama], Backend::Rocm);
        assert_eq!(devices[&Feature::Diffusion], Backend::Rocm);
    }

    #[test]
    fn cpu_torch_stays_on_cpu_for_a_vulkan_candidate() {
        let hardware = Hardware {
            devices: vec![Device::vulkan(0)],
            candidates: vec![0],
            selected: Some(0),
        };
        let (_, devices) = Runtime::plan(&[Feature::Torch], &hardware)
            .unwrap()
            .unwrap();

        assert_eq!(devices[&Feature::Torch], Backend::Cpu);
    }

    #[tokio::test]
    #[ignore = "requires an installed native runtime store"]
    async fn installed_torch_runtime_selects_a_usable_candidate() {
        let store = std::env::var_os("KOHARU_RUNTIME_SMOKE_STORE")
            .expect("KOHARU_RUNTIME_SMOKE_STORE must name an installed runtime store");
        crate::Store::configure(PathBuf::from(store)).unwrap();

        let devices = Runtime::discover([Feature::Torch])
            .unwrap()
            .initialize()
            .await
            .unwrap();
        let torch = devices.device(Feature::Torch).unwrap();
        eprintln!("selected Torch device: {}", torch.description);
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn rocm_hardware(target: &str, vulkan: bool) -> Hardware {
        let mut devices = vec![Device {
            index: 0,
            name: "ROCm0".to_owned(),
            description: target.to_owned(),
            backend: Backend::Rocm,
            device_type: DeviceType::IntegratedGpu,
            memory_total: 0,
            memory_free: 0,
            compute_capability: 0,
            target: Some(target.to_owned()),
        }];
        if vulkan {
            devices.push(Device::vulkan(0));
        }
        Hardware {
            devices,
            candidates: vec![0],
            selected: Some(0),
        }
    }
}
