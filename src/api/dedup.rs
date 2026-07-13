//! Cross-provider device de-duplication.
//!
//! A physical device reachable both natively (Hue, Govee, Sonos, Onkyo) and via
//! an integration (Home Assistant) imports as two rows. When both sides report
//! the same hardware id (`hw_id`, a normalized MAC), the reconciler **shadows**
//! the integration copy under the native one: the native device is canonical,
//! the integration copy stays tracked but drops out of control and room
//! membership and the Devices page collapses it. Native always wins — that's
//! Bifrost's reason to exist over HA.
//!
//! Matching is exact `hw_id` only (a MAC is unique), so auto-shadowing is safe.
//! Devices without a hw_id are never auto-touched; the manual link
//! ([`set_device_shadow`]) is the escape hatch for those.

use crate::AppState;
use crate::providers::ProviderDomain;
use axum::http::StatusCode;
use sqlx::Row;
use std::collections::HashMap;

/// The device-domain tables that carry `hw_id` / `shadowed_by`.
const TABLES: [&str; 4] = ["lights", "media_devices", "power_devices", "sensor_devices"];

/// Recompute automatic shadows across every device domain. Cheap and
/// idempotent — run it after any discovery or provider change.
pub(crate) async fn reconcile_duplicates(state: &AppState) {
    for table in TABLES {
        if let Err(e) = reconcile_table(state, table).await {
            tracing::error!("de-dup reconcile failed for {table}: {e}");
        }
    }
    // Shadowing knows nothing about composite groups, so a media row that just
    // got shadowed may be a group member — migrate its group (companions +
    // paired remotes) onto the canonical row and detach it, keeping the
    // "never both shadowed and grouped" invariant true.
    crate::api::media::enforce_shadow_group_exclusion(state).await;
}

async fn reconcile_table(state: &AppState, table: &str) -> sqlx::Result<()> {
    // Drop the previous run's auto-shadows so a removed or changed match
    // re-surfaces. Manual links (`shadow_auto = 0`) are left untouched.
    sqlx::query(&format!(
        "UPDATE {table} SET shadowed_by = NULL, shadow_auto = 0 WHERE shadow_auto = 1"
    ))
    .execute(&state.db)
    .await?;

    // Every device that advertises a hardware id, with its provider type.
    let rows = sqlx::query(&format!(
        "SELECT d.id AS id, d.hw_id AS hw_id, p.provider_type AS provider_type
         FROM {table} d JOIN providers p ON d.provider_id = p.id
         WHERE d.hw_id IS NOT NULL AND p.enabled = 1"
    ))
    .fetch_all(&state.db)
    .await?;

    // Cluster by hardware id; remember whether each member is from a native
    // (single-domain) provider or an integration.
    let mut clusters: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    for r in rows {
        let id: String = r.get("id");
        let hw: String = r.get("hw_id");
        let ptype: String = r.get("provider_type");
        let is_native = !matches!(
            state.registry.ui_domain(&ptype),
            Some(ProviderDomain::Integration)
        );
        clusters.entry(hw).or_default().push((id, is_native));
    }

    for (_hw, mut members) in clusters {
        if members.len() < 2 {
            continue;
        }
        // Deterministic canonical: a native member, lexically smallest id.
        members.sort_by(|a, b| a.0.cmp(&b.0));
        let Some(canonical) = members
            .iter()
            .find(|(_, native)| *native)
            .map(|(id, _)| id.clone())
        else {
            continue; // no native side (e.g. two HA instances) — nothing to prefer
        };
        // Shadow each integration copy under the native canonical, but only if
        // it's currently visible, so a manual link is never overwritten.
        for (id, is_native) in &members {
            if *is_native {
                continue;
            }
            let res = sqlx::query(&format!(
                "UPDATE {table} SET shadowed_by = ?, shadow_auto = 1
                 WHERE id = ? AND shadowed_by IS NULL"
            ))
            .bind(&canonical)
            .bind(id)
            .execute(&state.db)
            .await?;
            if res.rows_affected() > 0 {
                tracing::debug!(table, device = %id, canonical = %canonical, "de-dup: shadowed integration copy under native");
            }
        }
    }
    Ok(())
}

/// Manually set (or clear, with `None`) a device's shadow link — the no-hw_id
/// fallback and the user override. Recorded as a manual shadow (`shadow_auto =
/// 0`) so the reconciler leaves it alone. `table` is a fixed per-domain
/// identifier, so the formatted SQL is injection-free.
pub(crate) async fn set_device_shadow(
    state: &AppState,
    table: &str,
    id: &str,
    shadowed_by: Option<String>,
) -> StatusCode {
    if shadowed_by.as_deref() == Some(id) {
        return StatusCode::UNPROCESSABLE_ENTITY; // a device can't shadow itself
    }
    match sqlx::query(&format!(
        "UPDATE {table} SET shadowed_by = ?, shadow_auto = 0 WHERE id = ?"
    ))
    .bind(&shadowed_by)
    .bind(id)
    .execute(&state.db)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // A manually-shadowed media row may be a composite member — migrate
            // its group to the canonical row (same invariant as the reconciler).
            if table == "media_devices" && shadowed_by.is_some() {
                crate::api::media::enforce_shadow_group_exclusion(state).await;
            }
            crate::api::notify_inventory(state, table);
            StatusCode::NO_CONTENT
        }
        Ok(_) => StatusCode::NOT_FOUND,
        Err(e) => {
            tracing::error!("db error setting {table} shadow: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use crate::providers::default_registry;
    use sqlx::SqlitePool;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    use std::sync::Arc;

    async fn test_state() -> Arc<AppState> {
        let opts = SqliteConnectOptions::from_str(":memory:")
            .unwrap()
            .foreign_keys(true);
        let db = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        Arc::new(AppState::new(
            db,
            "test-secret-key-32-bytes-exactly",
            default_registry(),
        ))
    }

    /// Insert a provider of `provider_type` (e.g. `govee` native, `ha` integration).
    async fn seed_provider(state: &AppState, id: &str, provider_type: &str) {
        sqlx::query(
            "INSERT INTO providers (id, provider_type, name, credentials) VALUES (?, ?, ?, 'x')",
        )
        .bind(id)
        .bind(provider_type)
        .bind(provider_type)
        .execute(&state.db)
        .await
        .unwrap();
    }

    /// Insert a light row with an optional hardware id.
    async fn seed_light(state: &AppState, id: &str, provider_id: &str, hw: Option<&str>) {
        sqlx::query(
            "INSERT INTO lights (id, provider_id, device_id, name, hw_id) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(provider_id)
        .bind(id)
        .bind(id)
        .bind(hw)
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn shadowed_by(state: &AppState, id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT shadowed_by FROM lights WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn integration_copy_is_shadowed_by_native_on_hw_match() {
        let state = test_state().await;
        seed_provider(&state, "p-govee", "govee").await;
        seed_provider(&state, "p-ha", "ha").await;
        seed_light(&state, "l-native", "p-govee", Some("mac:aabbccddeeff")).await;
        seed_light(&state, "l-ha", "p-ha", Some("mac:aabbccddeeff")).await;

        reconcile_duplicates(&state).await;

        // Native wins: HA copy points at it, native stays visible.
        assert_eq!(
            shadowed_by(&state, "l-ha").await.as_deref(),
            Some("l-native")
        );
        assert_eq!(shadowed_by(&state, "l-native").await, None);
    }

    #[tokio::test]
    async fn no_native_side_means_no_shadow() {
        let state = test_state().await;
        seed_provider(&state, "p-ha", "ha").await;
        seed_provider(&state, "p-ha2", "ha").await;
        seed_light(&state, "l-a", "p-ha", Some("mac:aabbccddeeff")).await;
        seed_light(&state, "l-b", "p-ha2", Some("mac:aabbccddeeff")).await;

        reconcile_duplicates(&state).await;

        // Two integration copies, no native to prefer → both stay visible.
        assert_eq!(shadowed_by(&state, "l-a").await, None);
        assert_eq!(shadowed_by(&state, "l-b").await, None);
    }

    #[tokio::test]
    async fn mismatched_hw_ids_are_not_de_duped() {
        let state = test_state().await;
        seed_provider(&state, "p-govee", "govee").await;
        seed_provider(&state, "p-ha", "ha").await;
        seed_light(&state, "l-native", "p-govee", Some("mac:111111111111")).await;
        seed_light(&state, "l-ha", "p-ha", Some("mac:222222222222")).await;

        reconcile_duplicates(&state).await;

        assert_eq!(shadowed_by(&state, "l-ha").await, None);
    }

    #[tokio::test]
    async fn removing_native_resurfaces_the_integration_copy() {
        let state = test_state().await;
        seed_provider(&state, "p-govee", "govee").await;
        seed_provider(&state, "p-ha", "ha").await;
        seed_light(&state, "l-native", "p-govee", Some("mac:aabbccddeeff")).await;
        seed_light(&state, "l-ha", "p-ha", Some("mac:aabbccddeeff")).await;
        reconcile_duplicates(&state).await;
        assert_eq!(
            shadowed_by(&state, "l-ha").await.as_deref(),
            Some("l-native")
        );

        // The native provider (and its device) goes away…
        sqlx::query("DELETE FROM providers WHERE id = 'p-govee'")
            .execute(&state.db)
            .await
            .unwrap();
        reconcile_duplicates(&state).await;

        // …so the previously-shadowed HA copy is visible again.
        assert_eq!(shadowed_by(&state, "l-ha").await, None);
    }

    #[tokio::test]
    async fn manual_link_survives_reconcile() {
        let state = test_state().await;
        seed_provider(&state, "p-ha", "ha").await;
        // Two HA devices with no hw_id; the user links one under the other.
        seed_light(&state, "l-keep", "p-ha", None).await;
        seed_light(&state, "l-dupe", "p-ha", None).await;
        assert_eq!(
            set_device_shadow(&state, "lights", "l-dupe", Some("l-keep".into())).await,
            StatusCode::NO_CONTENT
        );

        reconcile_duplicates(&state).await;

        // The manual link (shadow_auto = 0) is untouched by reconciliation.
        assert_eq!(
            shadowed_by(&state, "l-dupe").await.as_deref(),
            Some("l-keep")
        );
    }

    #[tokio::test]
    async fn cannot_shadow_self() {
        let state = test_state().await;
        seed_provider(&state, "p-ha", "ha").await;
        seed_light(&state, "l-a", "p-ha", None).await;
        assert_eq!(
            set_device_shadow(&state, "lights", "l-a", Some("l-a".into())).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
