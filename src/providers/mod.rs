pub mod govee;
pub mod govee_lan;
pub mod hue;
pub mod shelly;
pub mod tasmota;
pub mod wled;

use crate::models::{Light, LightState};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashMap;

// ── Core provider trait ─────────────────────────────────────────────────────

/// A light group defined inside the provider's own ecosystem (e.g. a Hue
/// room or zone), mirrored locally and linkable from Bifrost Rooms.
#[derive(Debug, Clone)]
pub struct ProviderGroup {
    /// The group's id in the provider's namespace (e.g. Hue room UUID).
    pub provider_group_id: String,
    pub name: String,
    /// Device IDs of member lights, in the provider's namespace
    /// (matches `Light::provider_id` / the `lights.device_id` column).
    pub member_device_ids: Vec<String>,
    /// Native group-control handle (Hue grouped_light rid); None if the
    /// provider has no single-call group control.
    pub grouped_ref: Option<String>,
}

/// Runtime interface every provider must implement.
#[async_trait]
pub trait LightProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn discover(&self) -> Result<Vec<Light>>;
    async fn set_state(&self, device_id: &str, state: &LightState) -> Result<()>;
    async fn get_state(&self, device_id: &str) -> Result<LightState>;

    /// Groups (rooms/zones) defined in the provider's own ecosystem.
    /// Default: none — only providers with a native grouping concept override.
    async fn discover_groups(&self) -> Result<Vec<ProviderGroup>> {
        Ok(vec![])
    }

    /// Apply a state to a whole provider group in one native call, using the
    /// `grouped_ref` from `discover_groups`. Returns Ok(false) when the
    /// provider has no such mechanism (callers then fan out per light).
    async fn set_group_state(&self, _grouped_ref: &str, _state: &LightState) -> Result<bool> {
        Ok(false)
    }
}

// ── Credential schema (for the setup UI) ───────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct CredentialField {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Password,
    IpAddress,
    Url,
}

// ── Connection mode ─────────────────────────────────────────────────────────

/// How the runtime keeps a provider's light states fresh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionMode {
    /// Push events via the Hue CLIP v2 SSE stream, managed by `HueConnectionManager`.
    Sse,
    /// Periodic state polling via `PollingManager`.
    Poll { interval_secs: u64 },
}

/// Default polling cadence. Conservative enough for cloud APIs with daily
/// rate limits (Govee: 10 000 req/day).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 120;

// ── Factory trait ───────────────────────────────────────────────────────────

/// A factory knows how to construct one type of provider from its credentials.
/// Implement this — not `LightProvider` directly — when adding a new integration.
pub trait ProviderFactory: Send + Sync {
    /// The stable string key stored in the database (e.g. `"hue"`, `"govee"`).
    fn provider_type(&self) -> &'static str;

    /// Build a live provider from already-decrypted credentials JSON.
    fn build(&self, credentials_json: &str) -> Result<Box<dyn LightProvider>>;

    /// Describe the credential fields the UI must collect before calling `add_provider`.
    fn credentials_schema(&self) -> &'static [CredentialField];

    /// How the runtime should keep this provider's state fresh. Defaults to polling;
    /// only providers with a push channel need to override.
    fn connection_mode(&self) -> ConnectionMode {
        ConnectionMode::Poll {
            interval_secs: DEFAULT_POLL_INTERVAL_SECS,
        }
    }
}

// ── Registry ────────────────────────────────────────────────────────────────

/// Central registry. Add new providers once here; the rest of the app needs no changes.
pub struct ProviderRegistry {
    factories: HashMap<&'static str, Box<dyn ProviderFactory>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register<F: ProviderFactory + 'static>(&mut self, factory: F) {
        self.factories
            .insert(factory.provider_type(), Box::new(factory));
    }

    /// Build a live provider from a type string + decrypted credentials JSON.
    pub fn build(
        &self,
        provider_type: &str,
        credentials_json: &str,
    ) -> Result<Box<dyn LightProvider>> {
        self.factories
            .get(provider_type)
            .ok_or_else(|| anyhow!("unknown provider type: {provider_type}"))?
            .build(credentials_json)
    }

    /// Returns true if `provider_type` is registered.
    pub fn is_known(&self, provider_type: &str) -> bool {
        self.factories.contains_key(provider_type)
    }

    /// How the runtime should keep this provider type's state fresh.
    pub fn connection_mode(&self, provider_type: &str) -> Option<ConnectionMode> {
        self.factories
            .get(provider_type)
            .map(|f| f.connection_mode())
    }

    /// All registered provider types with their UI schemas, sorted by type name.
    pub fn all_types(&self) -> Vec<ProviderTypeInfo> {
        let mut types: Vec<_> = self
            .factories
            .values()
            .map(|f| ProviderTypeInfo {
                provider_type: f.provider_type(),
                schema: f.credentials_schema().to_vec(),
            })
            .collect();
        types.sort_by_key(|t| t.provider_type);
        types
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize)]
pub struct ProviderTypeInfo {
    pub provider_type: &'static str,
    pub schema: Vec<CredentialField>,
}

// ── Built-in provider registration ─────────────────────────────────────────

/// Returns a registry pre-loaded with every built-in provider.
/// Call this once in main; pass the result into AppState.
pub fn default_registry() -> ProviderRegistry {
    let mut r = ProviderRegistry::new();
    r.register(hue::HueProviderFactory);
    r.register(govee::GoveeProviderFactory);
    r.register(govee_lan::GoveeLanProviderFactory);
    r.register(shelly::ShellyProviderFactory);
    r.register(tasmota::TasmotaProviderFactory);
    r.register(wled::WledProviderFactory);
    r
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::models::{Light, LightState};
    use anyhow::Result;

    // ── Minimal mock provider for registry tests ────────────────────────────

    struct MockProvider;

    #[async_trait::async_trait]
    impl LightProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn discover(&self) -> Result<Vec<Light>> {
            Ok(vec![])
        }
        async fn set_state(&self, _id: &str, _s: &LightState) -> Result<()> {
            Ok(())
        }
        async fn get_state(&self, _id: &str) -> Result<LightState> {
            Ok(LightState::default())
        }
    }

    pub struct MockProviderFactory;

    impl ProviderFactory for MockProviderFactory {
        fn provider_type(&self) -> &'static str {
            "mock"
        }

        fn build(&self, _credentials_json: &str) -> Result<Box<dyn LightProvider>> {
            Ok(Box::new(MockProvider))
        }

        fn credentials_schema(&self) -> &'static [CredentialField] {
            &[CredentialField {
                name: "token",
                label: "Token",
                kind: FieldKind::Password,
                required: true,
                hint: None,
            }]
        }
    }

    // ── Registry tests ──────────────────────────────────────────────────────

    #[test]
    fn register_and_build_known_type() {
        let mut reg = ProviderRegistry::new();
        reg.register(MockProviderFactory);
        assert!(reg.build("mock", "{}").is_ok());
    }

    #[test]
    fn build_unknown_type_returns_error() {
        let reg = ProviderRegistry::new();
        let err = reg
            .build("nonexistent", "{}")
            .err()
            .expect("expected an error");
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn is_known_reflects_registered_types() {
        let mut reg = ProviderRegistry::new();
        assert!(!reg.is_known("mock"));
        reg.register(MockProviderFactory);
        assert!(reg.is_known("mock"));
    }

    #[test]
    fn all_types_sorted_alphabetically() {
        let reg = default_registry();
        let types = reg.all_types();
        let names: Vec<_> = types.iter().map(|t| t.provider_type).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn default_registry_contains_hue_and_govee() {
        let reg = default_registry();
        assert!(reg.is_known("hue"));
        assert!(reg.is_known("govee"));
    }

    #[test]
    fn default_registry_contains_govee_lan() {
        let reg = default_registry();
        assert!(reg.is_known("govee-lan"));
        // Builds from a bind address without touching the network.
        assert!(reg.build("govee-lan", r#"{"bind_addr":"0.0.0.0"}"#).is_ok());
    }

    #[test]
    fn default_registry_contains_wled() {
        let reg = default_registry();
        assert!(reg.is_known("wled"));
    }

    #[test]
    fn default_registry_contains_tasmota_and_shelly() {
        let reg = default_registry();
        assert!(reg.is_known("tasmota"));
        assert!(reg.is_known("shelly"));
    }

    #[test]
    fn hue_uses_sse_mode_all_others_poll() {
        let reg = default_registry();
        assert_eq!(reg.connection_mode("hue"), Some(ConnectionMode::Sse));
        for t in ["govee", "govee-lan", "wled", "tasmota", "shelly"] {
            match reg.connection_mode(t) {
                Some(ConnectionMode::Poll { interval_secs }) => {
                    assert!(interval_secs > 0, "{t}: zero poll interval")
                }
                other => panic!("{t}: expected Poll mode, got {other:?}"),
            }
        }
    }

    #[test]
    fn connection_mode_for_unknown_type_is_none() {
        let reg = ProviderRegistry::new();
        assert!(reg.connection_mode("nonexistent").is_none());
    }

    #[test]
    fn credentials_schema_describes_required_fields() {
        let reg = default_registry();
        let types = reg.all_types();
        for t in &types {
            assert!(
                !t.schema.is_empty(),
                "provider '{}' has no credential fields",
                t.provider_type
            );
            assert!(
                t.schema.iter().any(|f| f.required),
                "provider '{}' has no required fields",
                t.provider_type
            );
        }
    }
}
