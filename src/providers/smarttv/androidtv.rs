//! Generic **Android / Google TV** vendor — dongles, boxes, and any set that
//! speaks the Android TV Remote v2 protocol, with **no vendor HTTP API at
//! all**. Everything rides the one paired ATV session (`atv::client`):
//! keys, text, app launch (`RemoteAppLinkLaunchRequest`), and state — the
//! channel is push-only (the TV volunteers screen/volume/foreground-app and
//! front-loads them on connect), so snapshots read the link's state cache
//! rather than querying.
//!
//! Power and mute are physically **toggles** (`KEYCODE_POWER` / `_MUTE`);
//! Bifrost commands are absolute, so both are guarded by the cached state and
//! refuse (rather than blindly toggle) while the TV hasn't spoken yet.

use super::atv::{self, crypto::Identity};
use super::{TvIdentity, TvPush, TvSnapshot};
use crate::models::media::NowPlaying;
use crate::models::remote::{RemoteCommandInfo, RemoteKey, app_display_name, is_system_surface};
use anyhow::{Result, bail};
use async_trait::async_trait;

const KEY_POWER: u32 = 26;
const KEY_VOLUME_UP: u32 = 24;
const KEY_VOLUME_DOWN: u32 = 25;
const KEY_MUTE: u32 = 164;
/// Absolute volume is emulated with up/down steps; never walk further than
/// this in one command (a bogus cached level must not blast the volume).
const MAX_VOLUME_STEPS: i64 = 40;

pub(crate) struct AndroidTvVendor {
    host: String,
    /// Display name — stamped by discovery (mDNS) or the user; the protocol
    /// itself has no name query.
    name: Option<String>,
    /// Normalized hardware id (mDNS `bt` MAC) for cross-provider de-dup.
    hw_id: Option<String>,
    atv: Option<Identity>,
}

impl AndroidTvVendor {
    pub(crate) fn new(
        host: &str,
        name: Option<String>,
        hw_id: Option<String>,
        atv: Option<Identity>,
    ) -> Result<Self> {
        if host.trim().is_empty() {
            bail!("androidtv: empty host");
        }
        Ok(Self {
            host: host.trim().to_string(),
            name,
            hw_id,
            atv,
        })
    }

    fn identity_or_pair(&self) -> Result<&Identity> {
        self.atv.as_ref().ok_or_else(|| {
            anyhow::anyhow!("androidtv: remote not paired — pair it to control this TV")
        })
    }

    /// The link's cached state, retrying briefly: the ATV channel front-loads
    /// screen/volume pushes right after connect, so a just-started link fills
    /// within a moment.
    async fn settled_cache(&self, id: &Identity) -> atv::client::AtvStateCache {
        for _ in 0..6 {
            let c = atv::client::cached_state(&self.host, id);
            if c.screen_on.is_some() {
                return c;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        atv::client::cached_state(&self.host, id)
    }
}

#[async_trait]
impl super::SmartTvVendor for AndroidTvVendor {
    fn brand(&self) -> &'static str {
        "Android TV"
    }

    async fn identity(&self) -> Result<TvIdentity> {
        Ok(TvIdentity {
            name: self
                .name
                .clone()
                .unwrap_or_else(|| format!("Android TV ({})", self.host)),
            // Prefer the real hardware MAC (mDNS `bt=`). When it's absent (found
            // by the TCP-port sweep, e.g. WSL2 where mDNS can't cross), fall
            // back to a stable HOST-based id: the media and remote rows both
            // come from this same host, so a shared id lets them auto-pair
            // (AIO TV control) via the same hw_id reconciler the Bravia uses.
            // A `host:` scheme can't false-match a real `mac:` de-dup cluster.
            hw_id: self
                .hw_id
                .clone()
                .or_else(|| Some(format!("host:{}", self.host))),
        })
    }

    async fn snapshot(&self) -> Result<TvSnapshot> {
        let id = self.identity_or_pair()?;
        let c = self.settled_cache(id).await;
        let volume = match (c.volume_level, c.volume_max) {
            (Some(l), Some(m)) if m > 0 => ((l * 100).div_ceil(m) as u8).min(100),
            _ => 0,
        };
        // The foreground app IS this device's now-playing (there's no richer
        // metadata channel); system surfaces aren't content.
        let now_playing = c
            .current_app
            .as_deref()
            .filter(|p| c.screen_on == Some(true) && !is_system_surface(p))
            .map(|p| NowPlaying {
                title: Some(app_display_name(p)),
                artist: None,
                album: None,
                play_state: None,
                artwork_url: None,
            });
        Ok(TvSnapshot {
            // The TV has spoken on the link = reachable; a silent link means
            // it's off the network (or the pairing is dead).
            reachable: c.screen_on.is_some(),
            power: c.screen_on.unwrap_or(false),
            volume,
            mute: c.muted.unwrap_or(false),
            source: None,
            sources: Vec::new(),
            current_app: c.current_app,
            now_playing,
            ip: Some(self.host.clone()),
        })
    }

    async fn set_power(&self, on: bool) -> Result<()> {
        let id = self.identity_or_pair()?;
        let c = self.settled_cache(id).await;
        match c.screen_on {
            Some(current) if current == on => Ok(()), // absolute: already there
            Some(_) => atv::client::send_key(&self.host, id, KEY_POWER).await,
            // KEYCODE_POWER is a toggle — firing it blind could do the
            // opposite of what was asked. Refuse until the TV has spoken.
            None => bail!(
                "androidtv: screen state unknown yet (link still connecting) — try again in a moment"
            ),
        }
    }

    async fn set_volume(&self, percent: u8) -> Result<()> {
        let id = self.identity_or_pair()?;
        let c = atv::client::cached_state(&self.host, id);
        let (Some(level), Some(max)) = (c.volume_level, c.volume_max) else {
            bail!("androidtv: the TV hasn't reported its volume yet — try again in a moment");
        };
        if max == 0 {
            bail!("androidtv: TV reports no volume range");
        }
        // Emulate absolute volume with steps from the last-pushed level.
        let target = (u64::from(percent.min(100)) * u64::from(max)).div_ceil(100) as i64;
        let steps = (target - i64::from(level)).clamp(-MAX_VOLUME_STEPS, MAX_VOLUME_STEPS);
        let key = if steps >= 0 {
            KEY_VOLUME_UP
        } else {
            KEY_VOLUME_DOWN
        };
        for _ in 0..steps.unsigned_abs() {
            atv::client::send_key(&self.host, id, key).await?;
        }
        Ok(())
    }

    async fn set_mute(&self, mute: bool) -> Result<()> {
        let id = self.identity_or_pair()?;
        let c = atv::client::cached_state(&self.host, id);
        match c.muted {
            Some(current) if current == mute => Ok(()),
            Some(_) => atv::client::send_key(&self.host, id, KEY_MUTE).await,
            None => bail!("androidtv: mute state unknown yet — try again in a moment"),
        }
    }

    async fn set_source(&self, _source: &str) -> Result<()> {
        bail!("androidtv: no selectable inputs — launch an app instead")
    }

    async fn send_key(&self, key: RemoteKey) -> Result<()> {
        let id = self.identity_or_pair()?;
        atv::client::send_key(&self.host, id, atv::android_keycode(key)).await
    }

    async fn launch_app(&self, app: &str) -> Result<()> {
        let id = self.identity_or_pair()?;
        // `RemoteAppLinkLaunchRequest` takes a Play Store package id or a deep
        // link — pass through whatever the caller resolved.
        atv::client::send_message(&self.host, id, atv::messages::remote_app_link_launch(app))
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        let id = self.identity_or_pair()?;
        atv::client::send_text(&self.host, id, text).await
    }

    async fn commands(&self) -> Result<Vec<RemoteCommandInfo>> {
        Ok(Vec::new())
    }

    async fn push_stream(&self) -> Result<tokio::sync::mpsc::Receiver<TvPush>> {
        let id = self.identity_or_pair()?;
        Ok(super::atv_push_stream(&self.host, id))
    }

    async fn send_voice(&self, pcm_8k: &[u8]) -> Result<()> {
        let id = self.identity_or_pair()?;
        atv::client::send_voice(&self.host, id, pcm_8k).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::smarttv::SmartTvVendor as _;

    #[test]
    fn empty_host_is_rejected() {
        assert!(AndroidTvVendor::new("  ", None, None, None).is_err());
    }

    #[tokio::test]
    async fn identity_falls_back_to_a_host_hw_id_without_a_mac() {
        // No MAC (TCP-sweep discovery) → a stable host-based id so the media
        // and remote rows still share one → they auto-pair.
        let no_mac = AndroidTvVendor::new("192.168.1.44", None, None, None).unwrap();
        assert_eq!(
            no_mac.identity().await.unwrap().hw_id.as_deref(),
            Some("host:192.168.1.44")
        );
        // A real MAC (mDNS) always wins.
        let with_mac =
            AndroidTvVendor::new("192.168.1.44", None, Some("mac:bcdf586107a7".into()), None)
                .unwrap();
        assert_eq!(
            with_mac.identity().await.unwrap().hw_id.as_deref(),
            Some("mac:bcdf586107a7")
        );
    }

    #[tokio::test]
    async fn unpaired_vendor_refuses_control_but_identifies() {
        let v = AndroidTvVendor::new(
            "192.0.2.1",
            Some("Bedroom dongle".into()),
            Some("mac:bcdf586107a7".into()),
            None,
        )
        .unwrap();
        let id = v.identity().await.unwrap();
        assert_eq!(id.name, "Bedroom dongle");
        assert_eq!(id.hw_id.as_deref(), Some("mac:bcdf586107a7"));
        // Every control path needs the pairing.
        assert!(v.snapshot().await.is_err());
        assert!(v.send_key(RemoteKey::Home).await.is_err());
        assert!(v.launch_app("com.netflix.ninja").await.is_err());
        assert!(v.set_power(true).await.is_err());
    }

    #[tokio::test]
    async fn source_selection_points_at_apps() {
        let v = AndroidTvVendor::new("192.0.2.1", None, None, None).unwrap();
        let e = v.set_source("HDMI 1").await.unwrap_err().to_string();
        assert!(e.contains("launch an app"), "{e}");
    }
}
