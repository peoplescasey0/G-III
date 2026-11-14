//! Zero-trust sandbox isolation contract for Post Card Spatial OS.
//!
//! Ported from:
//! - `integrations/sandbox/nvidia-openshell/crates/openshell-isolation-interface/src/contract.rs`
//! - `integrations/sandbox/opensandbox`

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendErrorKind {
    Invalid,
    Denied,
    Unavailable,
    Unsupported,
    Failed,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    Descriptor(String),
    Denied(String),
    Unavailable(String),
    Unsupported(String),
    Attach(String),
    Confirm(String),
    Process(String),
    Terminated(String),
}

impl BackendError {
    pub fn kind(&self) -> BackendErrorKind {
        match self {
            Self::Descriptor(_) => BackendErrorKind::Invalid,
            Self::Denied(_) => BackendErrorKind::Denied,
            Self::Unavailable(_) => BackendErrorKind::Unavailable,
            Self::Unsupported(_) => BackendErrorKind::Unsupported,
            Self::Attach(_) | Self::Confirm(_) | Self::Process(_) => BackendErrorKind::Failed,
            Self::Terminated(_) => BackendErrorKind::Terminated,
        }
    }
}

/// Exact non-root workload identity validated before a sandbox is admitted
/// (`openshell-isolation-interface/src/contract.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkloadIdentity {
    pub uid: u32,
    pub gid: u32,
    pub supplementary_gids: Vec<u32>,
    pub source: String,
    pub resource_digest: String,
}

impl ResolvedWorkloadIdentity {
    pub fn new(
        uid: u32,
        gid: u32,
        mut supplementary_gids: Vec<u32>,
        source: &str,
        resource_digest: &str,
    ) -> Result<Self, BackendError> {
        if uid == 0 || gid == 0 || supplementary_gids.contains(&0) {
            return Err(BackendError::Descriptor(
                "workload identity must not contain UID or GID zero".to_string(),
            ));
        }
        if source.trim().is_empty() || resource_digest.trim().is_empty() {
            return Err(BackendError::Descriptor(
                "workload identity source and resource digest are required".to_string(),
            ));
        }
        supplementary_gids.retain(|&g| g != gid);
        supplementary_gids.sort_unstable();
        supplementary_gids.dedup();
        Ok(Self {
            uid,
            gid,
            supplementary_gids,
            source: source.to_string(),
            resource_digest: resource_digest.to_string(),
        })
    }
}

/// Generation-bound provisioning evidence (`openshell-isolation-interface/src/contract.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisioningEvidence {
    pub generation: String,
    pub evidence_digest: [u8; 32],
}

impl ProvisioningEvidence {
    pub fn from_enforcement_evidence(
        generation: &str,
        native_evidence: &[u8],
    ) -> Result<Self, BackendError> {
        if generation.is_empty() || native_evidence.is_empty() {
            return Err(BackendError::Descriptor(
                "provisioning generation and native evidence are required".to_string(),
            ));
        }
        let mut digest = [0u8; 32];
        // Deterministic 256-bit mixing across 4x64-bit FNV-1a lanes
        for (lane, chunk) in digest.chunks_exact_mut(8).enumerate() {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325u64.wrapping_add(lane as u64 * 0x9e37_79b9);
            for &b in generation.as_bytes().iter().chain(native_evidence.iter()) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            chunk.copy_from_slice(&h.to_be_bytes());
        }
        Ok(Self {
            generation: generation.to_string(),
            evidence_digest: digest,
        })
    }

    pub fn validate(&self, expected_generation: &str) -> Result<(), BackendError> {
        if !self.generation.is_empty() && self.generation == expected_generation {
            Ok(())
        } else {
            Err(BackendError::Confirm(
                "provisioning evidence belongs to another generation".to_string(),
            ))
        }
    }
}

/// Loopback-only target inside the sandbox boundary (`openshell-isolation-interface/src/contract.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackTarget {
    octets: [u8; 4],
    port: u16,
}

impl LoopbackTarget {
    pub fn new(octets: [u8; 4], port: u16) -> Result<Self, BackendError> {
        if octets[0] != 127 || port == 0 {
            return Err(BackendError::Process(format!(
                "port-forward target {}.{}.{}.{}:{} is not loopback",
                octets[0], octets[1], octets[2], octets[3], port
            )));
        }
        Ok(Self { octets, port })
    }

    pub fn octets(&self) -> [u8; 4] {
        self.octets
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Placement-neutral reason why a staged TCP open was denied (`contract.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpOpenDenial {
    PolicyDenied,
    IdentityUnavailable,
    InvalidDestination,
    ResourceExhausted,
    MediationUnavailable,
}

/// Typed supervisor decision for one pending TCP open (`contract.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpOpenDecision {
    RelayReady,
    Denied(TcpOpenDenial),
}

// ============================================================================
// PATENT PILLAR A: Risk-Based Augmented Secrets & Transitive Access Modulation
// ============================================================================

/// Distinguishes between Non-Secrets (intrinsic value) and Secrets (extrinsic/transitive value
/// derived from the assets or downstream secrets they unlock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetClass {
    NonSecret,
    Secret,
}

/// 1. Scope Metadata: tracks the blast radius (VMs, hosts, regions relying on the secret).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeMetadata {
    pub dependent_vms: u16,
    pub dependent_hosts: u16,
    pub cloud_regions: u8,
}

/// 2. Impact Metadata: qualitative/quantitative compromise damage score (0..=100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImpactMetadata {
    pub criticality_score: u8,
    pub unlocks_financial_or_root: bool,
}

/// 3. Exposure Metadata: tracks known exposure vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposureMetadata {
    pub transmitted_in_cleartext: bool,
    pub unguarded_volatile_ram: bool,
    pub copied_to_removable_media: bool,
    pub honeypot_canary: bool,
}

/// 4. Protection Metadata: security controls shielding the secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtectionMetadata {
    pub hardware_hsm_or_tpm_backed: bool,
    pub air_gapped: bool,
    pub strict_acl_enforced: bool,
    pub hw_boundary_guarded: bool,
}

/// 5. Rotation & Audit Metadata: lifecycle timestamps and immutable audit counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationAuditMetadata {
    pub created_tick: u64,
    pub last_rotated_tick: u64,
    pub expiration_tick: u64,
    pub audit_access_count: u32,
}

/// An Augmented Secret enriched with the 5-category risk metadata taxonomy and
/// transitive child-secret links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AugmentedSecret {
    pub secret_id: u32,
    pub label: String,
    pub asset_class: AssetClass,
    pub scope: ScopeMetadata,
    pub impact: ImpactMetadata,
    pub exposure: ExposureMetadata,
    pub protection: ProtectionMetadata,
    pub rotation: RotationAuditMetadata,
    /// IDs of downstream secrets unlocked by this secret (for Risk Transitivity).
    pub unlocks_secret_ids: Vec<u32>,
}

/// Dynamic access modulation decision computed from a secret's transitive risk profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModulationDecision {
    AllowDirect { risk_score: u32 },
    RequireStepUpAuth { risk_score: u32 },
    TriggerAutoRotationAndAllow { risk_score: u32, new_rotation_tick: u64 },
    BlockAndAlert { risk_score: u32, reason: &'static str },
}

impl AugmentedSecret {
    /// Computes the intrinsic base risk score of this secret from its 5 metadata categories.
    pub fn intrinsic_risk_score(&self) -> u32 {
        if self.asset_class == AssetClass::NonSecret {
            return 5;
        }
        let scope_pts = (self.scope.dependent_vms as u32) * 3
            + (self.scope.dependent_hosts as u32) * 5
            + (self.scope.cloud_regions as u32) * 10;

        let impact_pts = (self.impact.criticality_score as u32)
            + if self.impact.unlocks_financial_or_root { 40 } else { 0 };

        let mut exposure_pts = 0u32;
        if self.exposure.transmitted_in_cleartext {
            exposure_pts += 60;
        }
        if self.exposure.unguarded_volatile_ram {
            exposure_pts += 35;
        }
        if self.exposure.copied_to_removable_media {
            exposure_pts += 45;
        }
        if self.exposure.honeypot_canary {
            exposure_pts += 200;
        }

        let mut protection_reduction = 0u32;
        if self.protection.hardware_hsm_or_tpm_backed {
            protection_reduction += 25;
        }
        if self.protection.air_gapped {
            protection_reduction += 20;
        }
        if self.protection.strict_acl_enforced {
            protection_reduction += 15;
        }
        if self.protection.hw_boundary_guarded {
            protection_reduction += 15;
        }

        (scope_pts + impact_pts + exposure_pts).saturating_sub(protection_reduction)
    }
}

/// Evaluates an access request to `target.secret_id`, compounding **Risk Transitivity** across
/// any downstream secrets unlocked by `target` in `vault`, and returns a dynamic modulation decision.
pub fn evaluate_secret_access_with_transitivity(
    target: &mut AugmentedSecret,
    vault: &[AugmentedSecret],
    now_tick: u64,
) -> AccessModulationDecision {
    target.rotation.audit_access_count += 1;

    if target.exposure.honeypot_canary {
        return AccessModulationDecision::BlockAndAlert {
            risk_score: 999,
            reason: "honeypot canary secret touched",
        };
    }

    // Sum transitive risk from downstream secrets unlocked by this secret
    let mut transitive_bonus = 0u32;
    for child_id in &target.unlocks_secret_ids {
        if let Some(child) = vault.iter().find(|s| s.secret_id == *child_id) {
            transitive_bonus += (child.intrinsic_risk_score() * 60) / 100;
        }
    }

    let total_risk = target.intrinsic_risk_score() + transitive_bonus;
    let expired = now_tick >= target.rotation.expiration_tick;

    if total_risk >= 180 || target.exposure.transmitted_in_cleartext {
        AccessModulationDecision::BlockAndAlert {
            risk_score: total_risk,
            reason: "transitive blast radius or cleartext exposure exceeded threshold",
        }
    } else if expired {
        target.rotation.last_rotated_tick = now_tick;
        target.rotation.expiration_tick = now_tick + 10_000;
        AccessModulationDecision::TriggerAutoRotationAndAllow {
            risk_score: total_risk,
            new_rotation_tick: now_tick,
        }
    } else if total_risk >= 80 || !target.unlocks_secret_ids.is_empty() {
        AccessModulationDecision::RequireStepUpAuth {
            risk_score: total_risk,
        }
    } else {
        AccessModulationDecision::AllowDirect {
            risk_score: total_risk,
        }
    }
}

// ============================================================================
// PATENT PILLAR B: Credit-Card-Sized Modular Computer (CN108628404A)
// (Post Card Brain Module + Inert Peripheral Shell Dock)
// ============================================================================

/// The credit-card-sized portable computing "Brain" containing CPU, NVM flash, and
/// hardware encryption engine (`CN108628404A`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostCardBrainModule {
    pub card_serial: u64,
    pub dimensions_mm: (u8, u8, u8), // Standard credit card: (85, 54, 2) mm
    pub hardware_crypto_key_digest: [u8; 32],
    pub flash_capacity_mib: u32,
    pub docked_shell_id: Option<u32>,
}

/// Inert peripheral "Shell" reader equipped with display, keyboard, and power rails,
/// which becomes a functional computer only when a `PostCardBrainModule` is inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InertShellDock {
    pub shell_id: u32,
    pub display_width: u16,
    pub display_height: u16,
    pub has_keyboard: bool,
    pub loopback_bus_only: bool,
}

impl PostCardBrainModule {
    pub fn new_credit_card_brain(card_serial: u64, flash_capacity_mib: u32) -> Self {
        let ev = ProvisioningEvidence::from_enforcement_evidence("cn108628404a", b"postcard-brain")
            .expect("brain crypto key digest");
        Self {
            card_serial,
            dimensions_mm: (85, 54, 2),
            hardware_crypto_key_digest: ev.evidence_digest,
            flash_capacity_mib,
            docked_shell_id: None,
        }
    }

    /// Hot-plugs the credit-card Brain into an inert Shell reader after verifying
    /// loopback-only bus isolation.
    pub fn dock_into_shell(&mut self, dock: &InertShellDock) -> Result<u32, BackendError> {
        if !dock.loopback_bus_only {
            return Err(BackendError::Denied(
                "inert shell dock must enforce loopback-only peripheral bus".to_string(),
            ));
        }
        self.docked_shell_id = Some(dock.shell_id);
        Ok(dock.shell_id)
    }
}

