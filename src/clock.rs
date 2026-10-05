//! The hub's clock — NTP-corrected wall time, read in the hub's own time zone.
//!
//! Every time-of-day decision (kiosk hour plans, automation timers and time
//! gates, cooldowns, timed holds, session and enrollment expiry, `last_seen`
//! stamps) reads the clock through here — never `Utc::now()`, `Local::now()`
//! or SQLite's `datetime('now')` directly, since two clocks that disagree turn
//! into a stamp written on one and compared on the other.
//!
//! **Why not trust the host clock.** A hub runs in a container, which can't
//! discipline the host clock (that needs `CAP_SYS_TIME` and moves the whole
//! machine), and a board without an RTC boots at whatever time its image was
//! built. So the correction lives in-process: [`run`] measures the host clock
//! against SNTP servers and [`now_utc`] applies that offset. The host clock is
//! never touched. When no server answers, the last good offset stays in force
//! (a correction that was right an hour ago beats none), and a hub that has
//! never synced runs on the host clock, reported as such.
//!
//! **Why the zone is a setting.** A stock Docker deploy runs in UTC, and
//! nothing looks wrong until a 22:00 timer fires at 3pm. `config.timezone` (an
//! IANA name) is authoritative; empty defers to the process environment — a
//! `TZ` naming an IANA zone, else the system zone.

use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};
use std::time::Duration;

/// Servers queried when none are configured.
pub const DEFAULT_NTP_SERVERS: &[&str] = &["pool.ntp.org"];
/// Cap on configured servers, so a sync pass stays bounded.
pub const MAX_NTP_SERVERS: usize = 4;
/// Re-measure after a good sync. Crystal drift is seconds per day at worst,
/// so hourly keeps the error far below anything a minute-grained plan sees.
const RESYNC: Duration = Duration::from_secs(3600);
/// First retry after a failed pass, doubling up to [`RESYNC`].
const RETRY_MIN: Duration = Duration::from_secs(30);
/// Per-query deadline — a LAN or pool server answers in milliseconds.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);
/// Addresses sampled per configured name (a pool name resolves to several
/// independent servers, which is what makes the median meaningful).
const ADDRS_PER_NAME: usize = 3;
/// An offset change larger than this is logged at INFO — it moves schedules.
const NOTABLE_STEP_MS: i64 = 1000;
/// Seconds from the NTP era (1900-01-01) to the Unix epoch.
const NTP_UNIX_DELTA: i128 = 2_208_988_800;

/// The time zone the hub reads local time in.
#[derive(Clone, Debug, PartialEq)]
pub enum Zone {
    Named(Tz),
    /// The process's own zone (`chrono::Local` — `TZ` / `/etc/localtime`).
    System,
}

/// Where the effective [`Zone`] came from — shown so "why is it UTC" answers
/// itself.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneSource {
    Setting,
    Env,
    System,
}

/// The NTP side of the clock, as reported to the UI.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncStatus {
    /// NTP correction is switched on.
    pub enabled: bool,
    /// A sync has succeeded since boot (the offset is a measurement, not 0).
    pub synced: bool,
    /// The server whose sample set the offset (`name (ip)`).
    pub server: Option<String>,
    /// When the last good sample landed (corrected UTC, RFC 3339).
    pub last_sync: Option<String>,
    /// Round-trip delay of the chosen sample.
    pub delay_ms: Option<i64>,
    /// Why the most recent pass failed; cleared by the next success.
    pub last_error: Option<String>,
}

/// Everything the UI shows about the hub's clock.
#[derive(Clone, Debug, Serialize)]
pub struct ClockReport {
    /// Corrected local time, RFC 3339 with offset.
    pub now: String,
    /// Corrected time as Unix milliseconds — what a client derives its skew
    /// from.
    pub now_ms: i64,
    /// IANA name of the effective zone, when it has one.
    pub timezone: Option<String>,
    pub zone_source: ZoneSource,
    /// The zone's current UTC offset in minutes (DST-aware).
    pub utc_offset_min: i32,
    /// Correction applied to the host clock, in milliseconds.
    pub offset_ms: i64,
    pub ntp: SyncStatus,
}

/// One clock. The process has exactly one ([`clock`]); tests build their own.
pub struct Clock {
    offset_ms: AtomicI64,
    zone: RwLock<(Zone, ZoneSource)>,
    status: Mutex<SyncStatus>,
    poke: tokio::sync::Notify,
    attempted: tokio::sync::watch::Sender<bool>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            offset_ms: AtomicI64::new(0),
            zone: RwLock::new((Zone::System, ZoneSource::System)),
            status: Mutex::new(SyncStatus::default()),
            poke: tokio::sync::Notify::new(),
            attempted: tokio::sync::watch::channel(false).0,
        }
    }
}

impl Clock {
    /// Corrected UTC now.
    pub fn now_utc(&self) -> DateTime<Utc> {
        Utc::now() + chrono::Duration::milliseconds(self.offset_ms())
    }

    /// Corrected local now, in the hub's zone.
    pub fn now_local(&self) -> DateTime<FixedOffset> {
        self.to_local(self.now_utc())
    }

    /// A UTC instant in the hub's zone.
    pub fn to_local(&self, utc: DateTime<Utc>) -> DateTime<FixedOffset> {
        let zone = self.zone.read().unwrap_or_else(|e| e.into_inner());
        match &zone.0 {
            Zone::Named(tz) => utc.with_timezone(tz).fixed_offset(),
            Zone::System => utc.with_timezone(&chrono::Local).fixed_offset(),
        }
    }

    pub fn offset_ms(&self) -> i64 {
        self.offset_ms.load(Ordering::Relaxed)
    }

    pub fn set_zone(&self, zone: Zone, source: ZoneSource) {
        tracing::info!(target: "bifrost::clock", zone = %zone_label(&zone), source = ?source, "time zone set");
        *self.zone.write().unwrap_or_else(|e| e.into_inner()) = (zone, source);
    }

    /// IANA name of the effective zone, when it has one.
    pub fn zone_name(&self) -> Option<String> {
        match &self.zone.read().unwrap_or_else(|e| e.into_inner()).0 {
            Zone::Named(tz) => Some(tz.name().to_string()),
            Zone::System => None,
        }
    }

    pub fn report(&self) -> ClockReport {
        let now = self.now_utc();
        let local = self.to_local(now);
        let source = self.zone.read().unwrap_or_else(|e| e.into_inner()).1;
        ClockReport {
            now: local.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            now_ms: now.timestamp_millis(),
            timezone: self.zone_name(),
            zone_source: source,
            utc_offset_min: local.offset().local_minus_utc() / 60,
            offset_ms: self.offset_ms(),
            ntp: self
                .status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    /// Adopt a measured sample as the correction.
    fn apply(&self, sample: &Sample) {
        let prev = self.offset_ms.swap(sample.offset_ms, Ordering::Relaxed);
        if (sample.offset_ms - prev).abs() > NOTABLE_STEP_MS {
            tracing::info!(target: "bifrost::clock", offset_ms = sample.offset_ms, prev_ms = prev, server = %sample.server, "clock corrected");
        } else {
            tracing::debug!(target: "bifrost::clock", offset_ms = sample.offset_ms, delay_ms = sample.delay_ms, server = %sample.server, "ntp sync");
        }
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        st.enabled = true;
        st.synced = true;
        st.server = Some(sample.server.clone());
        st.delay_ms = Some(sample.delay_ms);
        st.last_sync = Some(
            self.now_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );
        st.last_error = None;
    }

    /// A failed pass: keep the last good offset, record why.
    fn fail(&self, error: String) {
        tracing::warn!(target: "bifrost::clock", %error, offset_ms = self.offset_ms(), "ntp sync failed — keeping the last correction");
        let mut st = self.status.lock().unwrap_or_else(|e| e.into_inner());
        st.enabled = true;
        st.last_error = Some(error);
    }

    /// NTP switched off: back to the host clock.
    fn disable(&self) {
        if self.offset_ms.swap(0, Ordering::Relaxed) != 0 {
            tracing::info!(target: "bifrost::clock", "ntp disabled — running on the host clock");
        }
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = SyncStatus::default();
    }

    /// Re-read the NTP settings and sync now (a settings edit).
    pub fn resync(&self) {
        self.poke.notify_one();
    }

    /// Resolve once the first sync attempt has finished (success or not).
    pub async fn first_attempt(&self) {
        let mut rx = self.attempted.subscribe();
        let _ = rx.wait_for(|done| *done).await;
    }
}

static CLOCK: LazyLock<Clock> = LazyLock::new(Clock::default);

/// The process clock.
pub fn clock() -> &'static Clock {
    &CLOCK
}

/// Corrected UTC now — use instead of `Utc::now()`.
pub fn now_utc() -> DateTime<Utc> {
    CLOCK.now_utc()
}

/// Corrected local now in the hub's zone — use instead of `Local::now()`.
pub fn now_local() -> DateTime<FixedOffset> {
    CLOCK.now_local()
}

/// Corrected now in SQLite's `datetime()` text form (UTC). Bind it where SQL
/// would otherwise say `datetime('now')`, which reads the uncorrected host
/// clock.
pub fn sql_now() -> String {
    sql_ts(now_utc())
}

/// A UTC instant in SQLite's `datetime()` text form.
pub fn sql_ts(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn zone_label(zone: &Zone) -> String {
    match zone {
        Zone::Named(tz) => tz.name().to_string(),
        Zone::System => "system".to_string(),
    }
}

/// Parse an IANA zone name (`America/Chicago`). A leading `:` (POSIX `TZ`
/// file syntax) is tolerated.
pub fn parse_zone(name: &str) -> Result<Tz, String> {
    let n = name.trim().trim_start_matches(':');
    n.parse::<Tz>()
        .map_err(|_| format!("'{}' is not an IANA time zone", name.trim()))
}

/// The effective zone: the hub setting, else an IANA `TZ`, else the system's.
/// An unparseable setting falls through rather than failing boot (it was
/// validated on write; a zone dropped from a newer tz database shouldn't
/// wedge the hub).
pub fn resolve_zone(setting: &str, env_tz: Option<&str>) -> (Zone, ZoneSource) {
    if !setting.trim().is_empty()
        && let Ok(tz) = parse_zone(setting)
    {
        return (Zone::Named(tz), ZoneSource::Setting);
    }
    if let Some(env) = env_tz.filter(|e| !e.trim().is_empty())
        && let Ok(tz) = parse_zone(env)
    {
        return (Zone::Named(tz), ZoneSource::Env);
    }
    (Zone::System, ZoneSource::System)
}

/// Every IANA zone name the hub accepts, sorted — the settings picker.
pub fn zone_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = chrono_tz::TZ_VARIANTS.iter().map(|tz| tz.name()).collect();
    names.sort_unstable();
    names
}

/// Validate one configured NTP server (`host`, `host:port`, an IP, or
/// `[v6]:port`), returning it trimmed.
pub fn normalize_server(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    let ok_chars = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '_'));
    if s.is_empty() || s.len() > 253 || !ok_chars {
        return Err(format!("'{}' is not a host name or address", raw.trim()));
    }
    match lookup_target(s).1 {
        Some(0) => return Err(format!("'{s}' has port 0")),
        None => return Err(format!("'{s}' has a bad port")),
        Some(_) => {}
    }
    Ok(s.to_string())
}

/// Split a server entry into what `lookup_host` wants. `.1` is `None` when a
/// port is present but unparseable.
fn lookup_target(s: &str) -> (String, Option<u16>) {
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return (sa.ip().to_string(), Some(sa.port()));
    }
    if let Ok(ip) = s.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        return (ip.to_string(), Some(123));
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host.to_string(), port.parse::<u16>().ok()),
        _ => (s.to_string(), Some(123)),
    }
}

// ── SNTP (RFC 4330) ─────────────────────────────────────────────────────────

/// Unix nanoseconds → a 64-bit NTP timestamp (32.32 fixed point since 1900).
fn to_ntp(unix_nanos: i128) -> u64 {
    let ntp_nanos = unix_nanos + NTP_UNIX_DELTA * 1_000_000_000;
    let secs = ntp_nanos.div_euclid(1_000_000_000);
    let frac = ntp_nanos.rem_euclid(1_000_000_000);
    ((secs as u64) << 32) | (((frac << 32) / 1_000_000_000) as u64)
}

/// A 64-bit NTP timestamp → Unix nanoseconds. Era per RFC 4330 §3: a clear
/// top bit means era 1 (2036 onward), so the mapping covers 1968–2104.
fn from_ntp(ts: u64) -> i128 {
    let mut secs = (ts >> 32) as i128;
    if secs & 0x8000_0000 == 0 {
        secs += 1 << 32;
    }
    let frac = ((ts & 0xffff_ffff) as i128 * 1_000_000_000) >> 32;
    (secs - NTP_UNIX_DELTA) * 1_000_000_000 + frac
}

/// A client request carrying `transmit` (our send time) for the server to
/// echo back as the originate timestamp.
fn build_request(transmit: u64) -> [u8; 48] {
    let mut buf = [0u8; 48];
    buf[0] = 0x23; // LI 0, version 4, mode 3 (client)
    buf[40..48].copy_from_slice(&transmit.to_be_bytes());
    buf
}

/// The server's receive (t2) and transmit (t3) timestamps.
#[derive(Debug, PartialEq)]
struct Reply {
    t2: u64,
    t3: u64,
}

fn read_ts(buf: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(buf[at..at + 8].try_into().expect("8 bytes"))
}

/// Validate a server reply against the request that carried `sent`.
fn parse_reply(buf: &[u8], sent: u64) -> Result<Reply, String> {
    if buf.len() < 48 {
        return Err(format!("short reply ({} bytes)", buf.len()));
    }
    let mode = buf[0] & 0x07;
    if mode != 4 {
        return Err(format!("not a server reply (mode {mode})"));
    }
    if buf[0] >> 6 == 3 {
        return Err("server clock is unsynchronised".into());
    }
    match buf[1] {
        0 => {
            let code = String::from_utf8_lossy(&buf[12..16]).trim().to_string();
            return Err(format!("server refused (kiss-o'-death {code})"));
        }
        s if s > 15 => return Err(format!("server is unsynchronised (stratum {s})")),
        _ => {}
    }
    // A reply that doesn't echo our transmit time answers someone else's
    // request (or is forged) — its timestamps say nothing about our clock.
    if read_ts(buf, 24) != sent {
        return Err("reply does not answer our request".into());
    }
    let (t2, t3) = (read_ts(buf, 32), read_ts(buf, 40));
    if t3 == 0 {
        return Err("reply has no transmit time".into());
    }
    Ok(Reply { t2, t3 })
}

/// Clock offset and round-trip delay (nanoseconds) from the four timestamps:
/// t1 client send, t2 server receive, t3 server send, t4 client receive.
fn offset_delay(t1: i128, t2: i128, t3: i128, t4: i128) -> (i128, i128) {
    (((t2 - t1) + (t3 - t4)) / 2, (t4 - t1) - (t3 - t2))
}

/// One measurement.
#[derive(Clone, Debug, PartialEq)]
struct Sample {
    offset_ms: i64,
    delay_ms: i64,
    server: String,
}

fn host_unix_nanos() -> i128 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0) as i128
}

/// Query one server address.
async fn query(addr: SocketAddr) -> Result<(i64, i64), String> {
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = tokio::net::UdpSocket::bind(bind)
        .await
        .map_err(|e| format!("bind: {e}"))?;
    sock.connect(addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let sent = to_ntp(host_unix_nanos());
    sock.send(&build_request(sent))
        .await
        .map_err(|e| format!("send: {e}"))?;
    let mut buf = [0u8; 128];
    let n = tokio::time::timeout(QUERY_TIMEOUT, sock.recv(&mut buf))
        .await
        .map_err(|_| "no reply".to_string())?
        .map_err(|e| format!("recv: {e}"))?;
    let t4 = host_unix_nanos();
    let reply = parse_reply(&buf[..n], sent)?;
    let (offset, delay) = offset_delay(from_ntp(sent), from_ntp(reply.t2), from_ntp(reply.t3), t4);
    Ok((
        (offset / 1_000_000) as i64,
        (delay / 1_000_000).max(0) as i64,
    ))
}

/// Choose the correction from a pass's samples: the median offset, so one
/// wrong server among several can't drag the clock (ties between the two
/// middle samples go to the shorter round trip — the tighter measurement).
fn pick(mut samples: Vec<Sample>) -> Option<Sample> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by_key(|s| s.offset_ms);
    let mid = samples.len() / 2;
    if samples.len().is_multiple_of(2) && samples[mid - 1].delay_ms <= samples[mid].delay_ms {
        return Some(samples.swap_remove(mid - 1));
    }
    Some(samples.swap_remove(mid))
}

/// One pass over the configured servers: resolve each, query its first few
/// addresses concurrently, and pick the correction.
async fn sample_servers(servers: &[String]) -> Result<Sample, String> {
    let mut set = tokio::task::JoinSet::new();
    let mut errors = Vec::new();
    for server in servers {
        let (host, port) = match lookup_target(server) {
            (host, Some(port)) => (host, port),
            (_, None) => {
                errors.push(format!("{server}: bad port"));
                continue;
            }
        };
        match tokio::net::lookup_host((host.as_str(), port)).await {
            Ok(addrs) => {
                for addr in addrs.take(ADDRS_PER_NAME) {
                    let label = if host == addr.ip().to_string() {
                        server.clone()
                    } else {
                        format!("{server} ({})", addr.ip())
                    };
                    set.spawn(async move { (label, query(addr).await) });
                }
            }
            Err(e) => errors.push(format!("{server}: {e}")),
        }
    }
    let mut samples = Vec::new();
    while let Some(Ok((server, result))) = set.join_next().await {
        match result {
            Ok((offset_ms, delay_ms)) => samples.push(Sample {
                offset_ms,
                delay_ms,
                server,
            }),
            Err(e) => errors.push(format!("{server}: {e}")),
        }
    }
    pick(samples).ok_or_else(|| {
        if errors.is_empty() {
            "no servers configured".to_string()
        } else {
            errors.join("; ")
        }
    })
}

/// The hub's clock settings.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockConfig {
    pub timezone: String,
    pub ntp_enabled: bool,
    pub ntp_servers: Vec<String>,
}

/// Read the clock settings; a missing config row (pre-setup) reads as the
/// defaults.
pub async fn read_config(db: &SqlitePool) -> ClockConfig {
    let row = sqlx::query("SELECT timezone, ntp_enabled, ntp_servers FROM config WHERE id = 1")
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    let Some(row) = row else {
        return ClockConfig {
            timezone: String::new(),
            ntp_enabled: true,
            ntp_servers: Vec::new(),
        };
    };
    ClockConfig {
        timezone: row.get("timezone"),
        ntp_enabled: row.get::<i64, _>("ntp_enabled") != 0,
        ntp_servers: row
            .get::<String, _>("ntp_servers")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
    }
}

/// The servers a pass queries: the configured list, else the defaults.
pub fn effective_servers(cfg: &ClockConfig) -> Vec<String> {
    if cfg.ntp_servers.is_empty() {
        DEFAULT_NTP_SERVERS.iter().map(|s| s.to_string()).collect()
    } else {
        cfg.ntp_servers.clone()
    }
}

/// Apply the stored zone setting to the process clock (boot, and every
/// settings edit).
pub async fn load_zone(db: &SqlitePool) {
    let cfg = read_config(db).await;
    let (zone, source) = resolve_zone(&cfg.timezone, std::env::var("TZ").ok().as_deref());
    CLOCK.set_zone(zone, source);
}

/// One sync pass on `clock`; returns how long to wait before the next.
async fn sync_pass(clock: &Clock, cfg: &ClockConfig, retry: &mut Duration) -> Duration {
    if !cfg.ntp_enabled {
        clock.disable();
        return RESYNC;
    }
    match sample_servers(&effective_servers(cfg)).await {
        Ok(sample) => {
            clock.apply(&sample);
            *retry = RETRY_MIN;
            RESYNC
        }
        Err(e) => {
            clock.fail(e);
            let wait = *retry;
            *retry = (*retry * 2).min(RESYNC);
            wait
        }
    }
}

/// The background sync loop (spawned once at boot). A settings edit
/// ([`Clock::resync`]) re-reads the config and syncs immediately.
pub async fn run(db: SqlitePool) {
    let mut retry = RETRY_MIN;
    loop {
        let cfg = read_config(&db).await;
        let wait = sync_pass(&CLOCK, &cfg, &mut retry).await;
        CLOCK.attempted.send_replace(true);
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = CLOCK.poke.notified() => retry = RETRY_MIN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    #[test]
    fn ntp_timestamps_roundtrip_through_unix_nanos() {
        let unix = 1_790_000_000_123_456_789i128; // 2026
        let back = from_ntp(to_ntp(unix));
        assert!((back - unix).abs() < 2, "{back} vs {unix}");
    }

    #[test]
    fn ntp_era_one_decodes_past_2036() {
        // 2040-01-01T00:00:00Z — NTP seconds have wrapped (top bit clear).
        let unix = Utc
            .with_ymd_and_hms(2040, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp() as i128;
        let ts = to_ntp(unix * 1_000_000_000);
        assert_eq!(ts >> 63, 0, "era 1 has the top bit clear");
        assert_eq!(from_ntp(ts) / 1_000_000_000, unix);
    }

    #[test]
    fn offset_and_delay_follow_rfc4330() {
        // Server 5s ahead, 100ms each way, 10ms turnaround.
        let s = 1_000_000_000i128;
        let t1 = 0;
        let t2 = 5 * s + s / 10;
        let t3 = t2 + s / 100;
        let t4 = t1 + s / 5 + s / 100;
        let (offset, delay) = offset_delay(t1, t2, t3, t4);
        assert_eq!(offset, 5 * s);
        assert_eq!(delay, s / 5);
    }

    fn reply(sent: u64, first: u8, stratum: u8) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0] = first;
        buf[1] = stratum;
        buf[24..32].copy_from_slice(&sent.to_be_bytes());
        buf[32..40].copy_from_slice(&7u64.to_be_bytes());
        buf[40..48].copy_from_slice(&9u64.to_be_bytes());
        buf
    }

    #[test]
    fn request_is_a_v4_client_packet_carrying_the_send_time() {
        let req = build_request(0xdead_beef_0000_0001);
        assert_eq!(req[0], 0x23);
        assert_eq!(read_ts(&req, 40), 0xdead_beef_0000_0001);
    }

    #[test]
    fn a_good_reply_yields_the_server_timestamps() {
        assert_eq!(
            parse_reply(&reply(42, 0x24, 2), 42),
            Ok(Reply { t2: 7, t3: 9 })
        );
    }

    #[test]
    fn replies_that_prove_nothing_are_rejected() {
        assert!(parse_reply(&reply(42, 0x24, 2)[..40], 42).is_err(), "short");
        assert!(parse_reply(&reply(42, 0x23, 2), 42).is_err(), "client mode");
        assert!(parse_reply(&reply(42, 0xe4, 2), 42).is_err(), "LI alarm");
        assert!(parse_reply(&reply(42, 0x24, 16), 42).is_err(), "stratum 16");
        assert!(
            parse_reply(&reply(41, 0x24, 2), 42).is_err(),
            "wrong originate"
        );
        let mut kod = reply(42, 0x24, 0);
        kod[12..16].copy_from_slice(b"RATE");
        let err = parse_reply(&kod, 42).unwrap_err();
        assert!(err.contains("RATE"), "{err}");
    }

    fn sample(offset_ms: i64, delay_ms: i64) -> Sample {
        Sample {
            offset_ms,
            delay_ms,
            server: format!("s{offset_ms}"),
        }
    }

    #[test]
    fn pick_takes_the_median_so_one_falseticker_cannot_drag_the_clock() {
        let picked = pick(vec![sample(10, 5), sample(3_600_000, 5), sample(12, 5)]).unwrap();
        assert_eq!(picked.offset_ms, 12);
        assert_eq!(pick(vec![]), None);
    }

    #[test]
    fn pick_breaks_an_even_split_on_the_shorter_round_trip() {
        assert_eq!(
            pick(vec![sample(10, 50), sample(20, 5)]).unwrap().offset_ms,
            20
        );
        assert_eq!(
            pick(vec![sample(10, 5), sample(20, 50)]).unwrap().offset_ms,
            10
        );
    }

    #[test]
    fn zone_resolution_prefers_setting_then_env_then_system() {
        let chicago: Tz = "America/Chicago".parse().unwrap();
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        assert_eq!(
            resolve_zone("America/Chicago", Some("Europe/Berlin")),
            (Zone::Named(chicago), ZoneSource::Setting)
        );
        assert_eq!(
            resolve_zone("", Some(":Europe/Berlin")),
            (Zone::Named(berlin), ZoneSource::Env)
        );
        assert_eq!(
            resolve_zone("", Some("CST6CDT,M3.2.0")).1,
            ZoneSource::System
        );
        assert_eq!(
            resolve_zone("Not/AZone", None),
            (Zone::System, ZoneSource::System)
        );
    }

    #[test]
    fn parse_zone_rejects_non_iana_names() {
        assert!(parse_zone("Asia/Tokyo").is_ok());
        assert!(parse_zone("Mars/Olympus").is_err());
    }

    #[test]
    fn zone_names_are_sorted_and_include_utc() {
        let names = zone_names();
        assert!(names.windows(2).all(|w| w[0] <= w[1]));
        assert!(names.contains(&"UTC"));
    }

    #[test]
    fn local_time_follows_the_named_zone_across_dst() {
        let clock = Clock::default();
        clock.set_zone(
            Zone::Named("America/New_York".parse().unwrap()),
            ZoneSource::Setting,
        );
        let winter = clock.to_local(Utc.with_ymd_and_hms(2026, 1, 15, 3, 30, 0).unwrap());
        assert_eq!((winter.day(), winter.hour()), (14, 22));
        let summer = clock.to_local(Utc.with_ymd_and_hms(2026, 7, 15, 3, 30, 0).unwrap());
        assert_eq!((summer.day(), summer.hour()), (14, 23));
        assert_eq!(clock.zone_name().as_deref(), Some("America/New_York"));
    }

    #[test]
    fn applying_a_sample_shifts_now_by_its_offset() {
        let clock = Clock::default();
        clock.apply(&sample(3_600_000, 4));
        let skew = (clock.now_utc() - Utc::now()).num_milliseconds();
        assert!((3_599_000..=3_601_000).contains(&skew), "{skew}");
        let report = clock.report();
        assert!(report.ntp.synced);
        assert_eq!(report.offset_ms, 3_600_000);
        assert_eq!(report.ntp.delay_ms, Some(4));
    }

    #[test]
    fn a_failed_pass_keeps_the_last_correction() {
        let clock = Clock::default();
        clock.apply(&sample(500, 4));
        clock.fail("no reply".into());
        assert_eq!(clock.offset_ms(), 500);
        let st = clock.report().ntp;
        assert!(st.synced);
        assert_eq!(st.last_error.as_deref(), Some("no reply"));
    }

    #[test]
    fn disabling_returns_to_the_host_clock() {
        let clock = Clock::default();
        clock.apply(&sample(500, 4));
        clock.disable();
        assert_eq!(clock.offset_ms(), 0);
        assert!(!clock.report().ntp.synced);
    }

    #[test]
    fn server_entries_are_validated() {
        assert_eq!(normalize_server(" pool.ntp.org ").unwrap(), "pool.ntp.org");
        assert!(normalize_server("192.168.1.1").is_ok());
        assert!(normalize_server("time.lan:1123").is_ok());
        assert!(normalize_server("[fe80::1]:123").is_ok());
        assert!(normalize_server("fe80::1").is_ok());
        assert!(normalize_server("").is_err());
        assert!(normalize_server("bad host").is_err());
        assert!(normalize_server("time.lan:notaport").is_err());
        assert!(normalize_server("time.lan:0").is_err());
    }

    #[test]
    fn sql_ts_matches_sqlite_datetime_text() {
        let t = Utc.with_ymd_and_hms(2026, 9, 30, 7, 5, 9).unwrap();
        assert_eq!(sql_ts(t), "2026-09-30 07:05:09");
    }

    /// A fake SNTP server on loopback that answers `ahead_ms` in the future.
    async fn fake_server(ahead_ms: i64) -> SocketAddr {
        let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = sock.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 48];
            while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
                if n < 48 {
                    continue;
                }
                let now = to_ntp(host_unix_nanos() + ahead_ms as i128 * 1_000_000);
                let mut out = [0u8; 48];
                out[0] = 0x24; // v4 server
                out[1] = 2;
                out[24..32].copy_from_slice(&buf[40..48]);
                out[32..40].copy_from_slice(&now.to_be_bytes());
                out[40..48].copy_from_slice(&now.to_be_bytes());
                let _ = sock.send_to(&out, peer).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn a_pass_measures_a_servers_offset() {
        let addr = fake_server(90_000).await;
        let sample = sample_servers(&[addr.to_string()]).await.unwrap();
        assert!((89_900..=90_100).contains(&sample.offset_ms), "{sample:?}");
    }

    #[tokio::test]
    async fn sync_pass_applies_a_good_sample_and_backs_off_a_bad_one() {
        let addr = fake_server(-30_000).await;
        let clock = Clock::default();
        let mut retry = RETRY_MIN;
        let cfg = ClockConfig {
            timezone: String::new(),
            ntp_enabled: true,
            ntp_servers: vec![addr.to_string()],
        };
        assert_eq!(sync_pass(&clock, &cfg, &mut retry).await, RESYNC);
        assert!((-30_100..=-29_900).contains(&clock.offset_ms()));

        // An address nothing answers on: keep the correction, back off.
        let dead = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cfg = ClockConfig {
            ntp_servers: vec![dead.local_addr().unwrap().to_string()],
            ..cfg
        };
        assert_eq!(sync_pass(&clock, &cfg, &mut retry).await, RETRY_MIN);
        assert_eq!(retry, RETRY_MIN * 2);
        assert!((-30_100..=-29_900).contains(&clock.offset_ms()));
        assert!(clock.report().ntp.last_error.is_some());
    }

    #[tokio::test]
    async fn sync_pass_disabled_runs_on_the_host_clock() {
        let clock = Clock::default();
        clock.apply(&sample(1234, 1));
        let mut retry = RETRY_MIN;
        let cfg = ClockConfig {
            timezone: String::new(),
            ntp_enabled: false,
            ntp_servers: vec![],
        };
        sync_pass(&clock, &cfg, &mut retry).await;
        assert_eq!(clock.offset_ms(), 0);
    }

    #[test]
    fn effective_servers_default_to_the_pool() {
        let cfg = ClockConfig {
            timezone: String::new(),
            ntp_enabled: true,
            ntp_servers: vec![],
        };
        assert_eq!(effective_servers(&cfg), vec!["pool.ntp.org".to_string()]);
    }
}
