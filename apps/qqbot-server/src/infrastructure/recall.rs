//! Encrypted, bounded and single-writer durable recall spool.
//!
//! WebSocket callbacks append authenticated encrypted frames locally. MySQL delivery happens only
//! in the worker. A tail torn by a crash is truncated to the last verified frame; completed-frame
//! corruption is quarantined and never silently accepted.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use fs2::FileExt;
use personal_secretary::{
    ConversationKind, ConversationRef, MessageSource, RecallCorrelationKey, RecallError,
    RecallEvent, RecallEventId, RecallFailureKind, RecallKind, RecallUseCase, SourceAccountRef,
};
use qqbot::napcat::{FriendRecallEvent, GroupRecallEvent, NapCatError};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::RecallWalConfig;
use crate::worker_lifecycle::WorkerHandle;

const RECALL_LEASE_SECS: u64 = 60;
const MAX_RECALL_ATTEMPTS: u32 = 12;
const MAGIC: &[u8; 4] = b"RSPL";
const VERSION: u8 = 1;
const SPOOL_HEADER_LEN: usize = 4 + 1 + 16;
const KEY_ID_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 4 + 1 + 4 + NONCE_LEN;
const TAG_LEN: usize = 16;
const MAX_FRAME_PLAINTEXT: usize = 16 * 1024;

const SPOOL_UNOBSERVED: u8 = 0;
const SPOOL_USABLE: u8 = 1;
const SPOOL_DEGRADED: u8 = 2;
const SPOOL_UNAVAILABLE: u8 = 3;
const SPOOL_ERROR_NONE: u8 = 0;
const SPOOL_ERROR_APPEND_FAILED: u8 = 1;
const SPOOL_ERROR_DRAIN_FAILED: u8 = 2;
const SPOOL_ERROR_RECOVERY_QUARANTINE: u8 = 3;
const SPOOL_ERROR_CAPACITY_EXHAUSTED: u8 = 4;

/// Recall Spool 的无敏感信息运行快照。数值均来自已验证 WAL 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecallSpoolSnapshot {
    pub observed: bool,
    pub usable: bool,
    pub degraded: bool,
    pub bytes_used: u64,
    pub capacity_bytes: u64,
    pub pending_frames: u64,
    pub oldest_occurred_at_unix_secs: Option<i64>,
    pub quarantine_count: u64,
    pub last_append_success_unix_secs: Option<i64>,
    pub last_drain_success_unix_secs: Option<i64>,
    pub recent_error_code: Option<&'static str>,
}

#[derive(Debug)]
pub struct RecallSpoolTelemetry {
    state: AtomicU8,
    bytes_used: AtomicU64,
    capacity_bytes: AtomicU64,
    pending_frames: AtomicU64,
    oldest_occurred_at_unix_secs: AtomicU64,
    quarantine_count: AtomicU64,
    last_append_success_unix_secs: AtomicU64,
    last_drain_success_unix_secs: AtomicU64,
    recent_error: AtomicU8,
    observed: AtomicBool,
}

impl RecallSpoolTelemetry {
    pub fn new(capacity_bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(SPOOL_UNOBSERVED),
            bytes_used: AtomicU64::new(0),
            capacity_bytes: AtomicU64::new(capacity_bytes),
            pending_frames: AtomicU64::new(0),
            oldest_occurred_at_unix_secs: AtomicU64::new(0),
            quarantine_count: AtomicU64::new(0),
            last_append_success_unix_secs: AtomicU64::new(0),
            last_drain_success_unix_secs: AtomicU64::new(0),
            recent_error: AtomicU8::new(SPOOL_ERROR_NONE),
            observed: AtomicBool::new(false),
        })
    }

    pub fn snapshot(&self) -> RecallSpoolSnapshot {
        let state = self.state.load(Ordering::Acquire);
        RecallSpoolSnapshot {
            observed: self.observed.load(Ordering::Acquire),
            usable: matches!(state, SPOOL_USABLE | SPOOL_DEGRADED),
            degraded: state == SPOOL_DEGRADED,
            bytes_used: self.bytes_used.load(Ordering::Acquire),
            capacity_bytes: self.capacity_bytes.load(Ordering::Acquire),
            pending_frames: self.pending_frames.load(Ordering::Acquire),
            oldest_occurred_at_unix_secs: nonzero_i64(
                self.oldest_occurred_at_unix_secs.load(Ordering::Acquire),
            ),
            quarantine_count: self.quarantine_count.load(Ordering::Acquire),
            last_append_success_unix_secs: nonzero_i64(
                self.last_append_success_unix_secs.load(Ordering::Acquire),
            ),
            last_drain_success_unix_secs: nonzero_i64(
                self.last_drain_success_unix_secs.load(Ordering::Acquire),
            ),
            recent_error_code: spool_error_code(self.recent_error.load(Ordering::Acquire)),
        }
    }

    fn mark_open(&self, bytes_used: u64, events: &[RecallEvent]) {
        self.observed.store(true, Ordering::Release);
        self.state.store(
            if self.recent_error.load(Ordering::Acquire) == SPOOL_ERROR_NONE {
                SPOOL_USABLE
            } else {
                SPOOL_DEGRADED
            },
            Ordering::Release,
        );
        self.update_backlog(bytes_used, events);
    }

    #[cfg(test)]
    pub(crate) fn set_test_snapshot(
        &self,
        bytes_used: u64,
        pending_frames: u64,
        quarantine_count: u64,
    ) {
        self.observed.store(true, Ordering::Release);
        self.state.store(SPOOL_USABLE, Ordering::Release);
        self.bytes_used.store(bytes_used, Ordering::Release);
        self.pending_frames.store(pending_frames, Ordering::Release);
        self.quarantine_count
            .store(quarantine_count, Ordering::Release);
    }

    fn mark_checkpoint(&self, bytes_used: u64, events: &[RecallEvent]) {
        self.last_drain_success_unix_secs
            .store(now_unix_u64(), Ordering::Release);
        self.recent_error.store(SPOOL_ERROR_NONE, Ordering::Release);
        self.state.store(SPOOL_USABLE, Ordering::Release);
        self.update_backlog(bytes_used, events);
    }

    fn mark_error(&self, code: u8, unavailable: bool) {
        self.observed.store(true, Ordering::Release);
        self.recent_error.store(code, Ordering::Release);
        self.state.store(
            if unavailable {
                SPOOL_UNAVAILABLE
            } else {
                SPOOL_DEGRADED
            },
            Ordering::Release,
        );
    }

    fn mark_quarantine(&self) {
        self.quarantine_count.fetch_add(1, Ordering::AcqRel);
        self.mark_error(SPOOL_ERROR_RECOVERY_QUARANTINE, false);
    }

    fn update_backlog(&self, bytes_used: u64, events: &[RecallEvent]) {
        self.bytes_used.store(bytes_used, Ordering::Release);
        self.pending_frames
            .store(events.len() as u64, Ordering::Release);
        let oldest = events
            .iter()
            .filter_map(|event| u64::try_from(event.occurred_at_unix_secs).ok())
            .min()
            .unwrap_or(0);
        self.oldest_occurred_at_unix_secs
            .store(oldest, Ordering::Release);
    }
}

fn spool_error_code(code: u8) -> Option<&'static str> {
    match code {
        SPOOL_ERROR_APPEND_FAILED => Some("spool_append_failed"),
        SPOOL_ERROR_DRAIN_FAILED => Some("spool_drain_failed"),
        SPOOL_ERROR_RECOVERY_QUARANTINE => Some("spool_recovery_quarantine"),
        SPOOL_ERROR_CAPACITY_EXHAUSTED => Some("spool_capacity_exhausted"),
        _ => None,
    }
}

fn nonzero_i64(value: u64) -> Option<i64> {
    (value > 0).then_some(value.min(i64::MAX as u64) as i64)
}

fn now_unix_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone)]
struct RecallWal {
    path: PathBuf,
    quarantine_dir: PathBuf,
    max_bytes: u64,
    key: Arc<aead::LessSafeKey>,
    key_id: [u8; KEY_ID_LEN],
    _lock_file: Arc<File>,
    lock: Arc<Mutex<()>>,
    telemetry: Arc<RecallSpoolTelemetry>,
}

impl RecallWal {
    fn open(
        config: &RecallWalConfig,
        telemetry: Arc<RecallSpoolTelemetry>,
    ) -> Result<Self, std::io::Error> {
        let (key, key_id) = load_key(&config.key_env)?;
        if let Some(parent) = config
            .path
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(&config.quarantine_dir)?;
        let lock_path = config.path.with_extension("lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock_file.try_lock_exclusive().map_err(|error| {
            std::io::Error::new(
                error.kind(),
                "recall spool is already locked by another process",
            )
        })?;
        let wal = Self {
            path: config.path.clone(),
            quarantine_dir: config.quarantine_dir.clone(),
            max_bytes: config.max_bytes,
            key: Arc::new(key),
            key_id,
            _lock_file: Arc::new(lock_file),
            lock: Arc::new(Mutex::new(())),
            telemetry,
        };
        wal.initialize_or_verify_header()?;
        wal.recover()?;
        wal.refresh_telemetry()?;
        if wal.telemetry.snapshot().quarantine_count > 0 {
            wal.telemetry
                .mark_error(SPOOL_ERROR_RECOVERY_QUARANTINE, false);
        }
        Ok(wal)
    }

    fn initialize_or_verify_header(&self) -> Result<(), std::io::Error> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| std::io::Error::other("recall spool lock poisoned"))?;
        let metadata = std::fs::metadata(&self.path).ok();
        if metadata.is_none_or(|value| value.len() == 0) {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&self.path)?;
            file.write_all(MAGIC)?;
            file.write_all(&[VERSION])?;
            file.write_all(&self.key_id)?;
            return file.sync_all();
        }
        let mut header = [0_u8; SPOOL_HEADER_LEN];
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        if file.read_exact(&mut header).is_err() || &header[..4] != MAGIC || header[4] != VERSION {
            return Err(std::io::Error::other(
                "recall spool header is missing or unsupported",
            ));
        }
        if header[5..] != self.key_id {
            return Err(std::io::Error::other(
                "recall spool encryption key does not match spool key id",
            ));
        }
        Ok(())
    }

    fn append(&self, event: &RecallEvent) -> Result<(), String> {
        let result = (|| {
            let plaintext = serde_json::to_vec(event).map_err(|error| error.to_string())?;
            if plaintext.len() > MAX_FRAME_PLAINTEXT {
                return Err("recall spool record exceeds maximum frame size".into());
            }
            let frame = self.encrypt(&plaintext)?;
            let _guard = self.lock.lock().map_err(|_| "recall spool lock poisoned")?;
            let current = std::fs::metadata(&self.path)
                .map_err(|error| error.to_string())?
                .len();
            if current.saturating_add(frame.len() as u64) > self.max_bytes {
                return Err("recall spool capacity exhausted".into());
            }
            let mut file = OpenOptions::new()
                .append(true)
                .open(&self.path)
                .map_err(|error| error.to_string())?;
            file.write_all(&frame).map_err(|error| error.to_string())?;
            file.sync_data().map_err(|error| error.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.refresh_telemetry()
                    .map_err(|error| error.to_string())?;
                self.telemetry
                    .last_append_success_unix_secs
                    .store(now_unix_u64(), Ordering::Release);
                Ok(())
            }
            Err(error) => {
                self.telemetry.mark_error(
                    if error == "recall spool capacity exhausted" {
                        SPOOL_ERROR_CAPACITY_EXHAUSTED
                    } else {
                        SPOOL_ERROR_APPEND_FAILED
                    },
                    error == "recall spool capacity exhausted",
                );
                Err(error)
            }
        }
    }

    fn refresh_telemetry(&self) -> Result<(), std::io::Error> {
        let bytes_used = std::fs::metadata(&self.path)?.len();
        let snapshot = self.snapshot().map_err(std::io::Error::other)?;
        let events = self.events(&snapshot).map_err(std::io::Error::other)?;
        self.telemetry.mark_open(bytes_used, &events);
        Ok(())
    }

    fn snapshot(&self) -> Result<Vec<u8>, String> {
        let _guard = self.lock.lock().map_err(|_| "recall spool lock poisoned")?;
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|error| error.to_string())?
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() < SPOOL_HEADER_LEN {
            return Err("recall spool header is truncated".into());
        }
        Ok(bytes[SPOOL_HEADER_LEN..].to_vec())
    }

    fn events(&self, snapshot: &[u8]) -> Result<Vec<RecallEvent>, String> {
        self.decode_frames(snapshot)
            .map(|frames| frames.into_iter().map(|(_, event)| event).collect())
    }

    fn remove_snapshot_prefix(&self, snapshot: &[u8]) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "recall spool lock poisoned")?;
        let mut current = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|error| error.to_string())?
            .read_to_end(&mut current)
            .map_err(|error| error.to_string())?;
        if current.len() < SPOOL_HEADER_LEN || !current[SPOOL_HEADER_LEN..].starts_with(snapshot) {
            self.telemetry.mark_error(SPOOL_ERROR_DRAIN_FAILED, false);
            return Err("recall spool changed before checkpoint".into());
        }
        self.replace(&current[SPOOL_HEADER_LEN + snapshot.len()..])?;
        drop(_guard);
        let bytes_used = std::fs::metadata(&self.path)
            .map_err(|error| error.to_string())?
            .len();
        let remaining = self.snapshot()?;
        let events = self.events(&remaining)?;
        self.telemetry.mark_checkpoint(bytes_used, &events);
        Ok(())
    }

    fn recover(&self) -> Result<(), std::io::Error> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| std::io::Error::other("recall spool lock poisoned"))?;
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&self.path)?
            .read_to_end(&mut bytes)?;
        let mut offset = SPOOL_HEADER_LEN;
        let mut recovered = Vec::new();
        while offset < bytes.len() {
            match self.decode_one(&bytes[offset..]) {
                Ok((consumed, _)) => {
                    recovered.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                }
                Err(FrameError::TornTail) => {
                    self.quarantine(&bytes[offset..], "torn_tail")?;
                    break;
                }
                Err(error) => {
                    let next = next_frame_offset(&bytes, offset);
                    let end = next.unwrap_or(bytes.len());
                    self.quarantine(&bytes[offset..end], error.code())?;
                    offset = end;
                }
            }
        }
        if recovered != bytes[SPOOL_HEADER_LEN..] {
            self.replace_io(&recovered)?;
        }
        Ok(())
    }

    fn decode_frames(&self, bytes: &[u8]) -> Result<Vec<(usize, RecallEvent)>, String> {
        let mut offset = 0;
        let mut result = Vec::new();
        while offset < bytes.len() {
            match self.decode_one(&bytes[offset..]) {
                Ok((consumed, event)) => {
                    result.push((consumed, event));
                    offset += consumed;
                }
                Err(error) => {
                    return Err(format!(
                        "recall spool invalid after recovery: {}",
                        error.code()
                    ));
                }
            }
        }
        Ok(result)
    }

    fn decode_one(&self, bytes: &[u8]) -> Result<(usize, RecallEvent), FrameError> {
        if bytes.len() < HEADER_LEN {
            return Err(FrameError::TornTail);
        }
        if &bytes[..4] != MAGIC {
            return Err(FrameError::InvalidMagic);
        }
        if bytes[4] != VERSION {
            return Err(FrameError::UnsupportedVersion);
        }
        let length =
            u32::from_be_bytes(bytes[5..9].try_into().expect("fixed frame header")) as usize;
        if !(TAG_LEN..=MAX_FRAME_PLAINTEXT + TAG_LEN).contains(&length) {
            return Err(FrameError::InvalidLength);
        }
        let total = HEADER_LEN
            .checked_add(length)
            .ok_or(FrameError::InvalidLength)?;
        if bytes.len() < total {
            return Err(FrameError::TornTail);
        }
        let nonce = aead::Nonce::try_assume_unique_for_key(&bytes[9..HEADER_LEN])
            .map_err(|_| FrameError::InvalidNonce)?;
        let mut ciphertext = bytes[HEADER_LEN..total].to_vec();
        let plaintext = self
            .key
            .open_in_place(nonce, aead::Aad::from(&bytes[..9]), &mut ciphertext)
            .map_err(|_| FrameError::Authentication)?;
        let event = serde_json::from_slice(plaintext).map_err(|_| FrameError::InvalidPayload)?;
        Ok((total, event))
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let mut nonce_bytes = [0_u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| "secure random unavailable")?;
        let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);
        let encrypted_len = plaintext
            .len()
            .checked_add(TAG_LEN)
            .ok_or("recall spool frame overflow")?;
        let mut output = Vec::with_capacity(HEADER_LEN + encrypted_len);
        output.extend_from_slice(MAGIC);
        output.push(VERSION);
        output.extend_from_slice(&(encrypted_len as u32).to_be_bytes());
        output.extend_from_slice(&nonce_bytes);
        let aad = aead::Aad::from(&output[..9]);
        let mut ciphertext = plaintext.to_vec();
        self.key
            .seal_in_place_append_tag(nonce, aad, &mut ciphertext)
            .map_err(|_| "recall spool encryption failed")?;
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    fn replace(&self, bytes: &[u8]) -> Result<(), String> {
        self.replace_io(bytes).map_err(|error| error.to_string())
    }

    fn replace_io(&self, bytes: &[u8]) -> Result<(), std::io::Error> {
        let temporary = self.path.with_extension("spool.tmp");
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        output.write_all(MAGIC)?;
        output.write_all(&[VERSION])?;
        output.write_all(&self.key_id)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        drop(output);
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }

    fn quarantine(&self, bytes: &[u8], reason: &str) -> Result<(), std::io::Error> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = self
            .quarantine_dir
            .join(format!("recall-spool-{timestamp}-{reason}.bin"));
        let result = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .and_then(|mut file| {
                file.write_all(bytes)?;
                file.sync_all()
            });
        if result.is_ok() {
            self.telemetry.mark_quarantine();
        }
        result
    }
}

fn next_frame_offset(bytes: &[u8], current_offset: usize) -> Option<usize> {
    let search_from = current_offset.saturating_add(1);
    bytes[search_from..]
        .windows(MAGIC.len())
        .position(|window| window == MAGIC)
        .map(|relative| search_from + relative)
}

fn load_key(
    environment_name: &str,
) -> Result<(aead::LessSafeKey, [u8; KEY_ID_LEN]), std::io::Error> {
    let encoded = std::env::var(environment_name)
        .map_err(|_| std::io::Error::other("recall spool encryption key is missing"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| std::io::Error::other("recall spool encryption key is not base64"))?;
    if bytes.len() != 32 {
        return Err(std::io::Error::other(
            "recall spool encryption key must decode to 32 bytes",
        ));
    }
    let mut key_id = [0_u8; KEY_ID_LEN];
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    key_id.copy_from_slice(&digest.as_ref()[..KEY_ID_LEN]);
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &bytes)
        .map_err(|_| std::io::Error::other("recall spool encryption key is invalid"))?;
    Ok((aead::LessSafeKey::new(key), key_id))
}

enum FrameError {
    TornTail,
    InvalidMagic,
    UnsupportedVersion,
    InvalidLength,
    InvalidNonce,
    Authentication,
    InvalidPayload,
}
impl FrameError {
    fn code(&self) -> &'static str {
        match self {
            Self::TornTail => "torn_tail",
            Self::InvalidMagic => "invalid_magic",
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidLength => "invalid_length",
            Self::InvalidNonce => "invalid_nonce",
            Self::Authentication => "authentication_failed",
            Self::InvalidPayload => "invalid_payload",
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use tempfile::TempDir;

    use super::*;

    fn config(temp: &TempDir, key: [u8; 32]) -> RecallWalConfig {
        let key_env = format!("QQBOT_RECALL_SPOOL_UNIT_{}", Uuid::new_v4().simple());
        // Tests use a unique process environment key and do not run environment mutations in
        // parallel with a spool open using the same name.
        unsafe {
            std::env::set_var(
                &key_env,
                base64::engine::general_purpose::STANDARD.encode(key),
            );
        }
        RecallWalConfig {
            path: temp.path().join("recall.spool"),
            quarantine_dir: temp.path().join("quarantine"),
            max_bytes: 1024 * 1024,
            drain_interval_ms: 10,
            key_env,
        }
    }

    fn event(message_id: &str) -> RecallEvent {
        let account = SourceAccountRef::new(MessageSource::NapCat, "account-1").unwrap();
        RecallEvent {
            recall_event_id: RecallEventId::new(Uuid::new_v4().to_string()).unwrap(),
            account: account.clone(),
            kind: RecallKind::Group,
            correlation: RecallCorrelationKey::new(
                account,
                MessageSource::NapCat,
                ConversationRef::new(ConversationKind::Group, "group-123").unwrap(),
                message_id,
            )
            .unwrap(),
            operator_platform_id: Some("operator-1".into()),
            occurred_at_unix_secs: 1,
        }
    }

    #[test]
    fn spool_ciphertext_hides_recall_identifiers_and_is_exclusive() {
        let temp = TempDir::new().unwrap();
        let config = config(&temp, [9; 32]);
        let spool = RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        spool.append(&event("message-456")).unwrap();
        let bytes = std::fs::read(&config.path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("account-1"));
        assert!(!text.contains("group-123"));
        assert!(!text.contains("message-456"));
        assert!(RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).is_err());
        drop(spool);
        assert!(RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).is_ok());
    }

    #[test]
    fn wrong_key_does_not_mutate_spool() {
        let temp = TempDir::new().unwrap();
        let good = config(&temp, [5; 32]);
        let spool = RecallWal::open(&good, RecallSpoolTelemetry::new(good.max_bytes)).unwrap();
        spool.append(&event("must-survive-wrong-key")).unwrap();
        let before = std::fs::read(&good.path).unwrap();
        drop(spool);

        let wrong = config(&temp, [6; 32]);
        assert!(RecallWal::open(&wrong, RecallSpoolTelemetry::new(wrong.max_bytes)).is_err());
        assert_eq!(std::fs::read(&good.path).unwrap(), before);
        assert!(RecallWal::open(&good, RecallSpoolTelemetry::new(good.max_bytes)).is_ok());
    }

    #[test]
    fn spool_rejects_capacity_before_acknowledgement() {
        let temp = TempDir::new().unwrap();
        let mut config = config(&temp, [4; 32]);
        config.max_bytes = 1;
        let spool = RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        assert!(spool.append(&event("capacity")).is_err());
        assert!(spool.snapshot().unwrap().is_empty());
    }

    #[test]
    fn spool_telemetry_reports_backlog_capacity_and_quarantine() {
        let temp = TempDir::new().unwrap();
        let config = config(&temp, [7; 32]);
        let telemetry = RecallSpoolTelemetry::new(config.max_bytes);
        let spool = RecallWal::open(&config, Arc::clone(&telemetry)).unwrap();
        spool.append(&event("telemetry-1")).unwrap();
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.pending_frames, 1);
        assert!(snapshot.bytes_used > SPOOL_HEADER_LEN as u64);
        assert_eq!(snapshot.capacity_bytes, config.max_bytes);
        assert_eq!(snapshot.oldest_occurred_at_unix_secs, Some(1));
        assert_eq!(snapshot.recent_error_code, None);

        let mut corrupted = std::fs::read(&config.path).unwrap();
        *corrupted.last_mut().unwrap() ^= 0x01;
        std::fs::write(&config.path, corrupted).unwrap();
        drop(spool);
        let recovered = RecallWal::open(&config, Arc::clone(&telemetry)).unwrap();
        let recovered_snapshot = telemetry.snapshot();
        assert_eq!(recovered_snapshot.pending_frames, 0);
        assert!(recovered_snapshot.quarantine_count >= 1);
        assert_eq!(
            recovered_snapshot.recent_error_code,
            Some("spool_recovery_quarantine")
        );
        drop(recovered);
    }

    #[test]
    fn spool_recovers_torn_tail_and_quarantines_authenticated_corruption() {
        let temp = TempDir::new().unwrap();
        let config = config(&temp, [3; 32]);
        let spool = RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        spool.append(&event("first")).unwrap();
        let original = std::fs::read(&config.path).unwrap();
        let mut torn = original.clone();
        torn.extend_from_slice(&[1, 2, 3]);
        std::fs::write(&config.path, torn).unwrap();
        drop(spool);
        let recovered =
            RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        assert_eq!(
            recovered
                .events(&recovered.snapshot().unwrap())
                .unwrap()
                .len(),
            1
        );
        drop(recovered);

        let mut corrupted = original.clone();
        *corrupted.last_mut().unwrap() ^= 0x01;
        std::fs::write(&config.path, corrupted).unwrap();
        let recovered =
            RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        assert!(recovered.snapshot().unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(&config.quarantine_dir).unwrap().count(),
            2
        );
        drop(recovered);

        let spool = RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        spool.append(&event("kept-before-corruption")).unwrap();
        spool.append(&event("kept-after-corruption")).unwrap();
        let mut mixed = std::fs::read(&config.path).unwrap();
        let second_frame_offset = next_frame_offset(&mixed, SPOOL_HEADER_LEN).unwrap();
        mixed[second_frame_offset + HEADER_LEN] ^= 0x01;
        let valid_second = spool
            .encrypt(&serde_json::to_vec(&event("survives")).unwrap())
            .unwrap();
        mixed.extend_from_slice(&valid_second);
        std::fs::write(&config.path, mixed).unwrap();
        drop(spool);
        let recovered =
            RecallWal::open(&config, RecallSpoolTelemetry::new(config.max_bytes)).unwrap();
        let events = recovered.events(&recovered.snapshot().unwrap()).unwrap();
        assert_eq!(
            events.len(),
            2,
            "valid frames surrounding corruption survive recovery"
        );
    }
}

#[derive(Clone)]
pub struct RecallQueue {
    wal: RecallWal,
    wake: Arc<Notify>,
}
impl RecallQueue {
    pub async fn enqueue(&self, event: RecallEvent) -> Result<(), NapCatError> {
        let recall_event_id = event.recall_event_id.as_str().to_owned();
        let wal = self.wal.clone();
        tokio::task::spawn_blocking(move || wal.append(&event))
            .await
            .map_err(|error| {
                error!(recall_event_id, error = %error, "撤回 spool 阻塞写入任务中断");
                NapCatError::Handler("personal secretary recall spool writer failed".into())
            })?
            .map_err(|error| {
                error!(recall_event_id, error, "撤回事件未能持久化进入加密 spool");
                NapCatError::Handler("personal secretary recall spool append failed".into())
            })?;
        self.wake.notify_one();
        debug!(recall_event_id, "撤回事件已同步写入加密 spool");
        Ok(())
    }
}

pub struct RecallWorkerHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}
impl RecallWorkerHandle {
    pub fn signal_and_detach(self) -> WorkerHandle {
        let _ = self.shutdown.send(true);
        WorkerHandle::new("recall_inbox", self.join)
    }
}

pub fn spawn_recall_worker(
    use_case: Arc<RecallUseCase>,
    config: RecallWalConfig,
) -> Result<(RecallQueue, RecallWorkerHandle), std::io::Error> {
    spawn_recall_worker_with_telemetry(
        use_case,
        config.clone(),
        RecallSpoolTelemetry::new(config.max_bytes),
    )
}

pub fn spawn_recall_worker_with_telemetry(
    use_case: Arc<RecallUseCase>,
    config: RecallWalConfig,
    telemetry: Arc<RecallSpoolTelemetry>,
) -> Result<(RecallQueue, RecallWorkerHandle), std::io::Error> {
    let wal = RecallWal::open(&config, telemetry)?;
    let wake = Arc::new(Notify::new());
    let (shutdown, receiver) = watch::channel(false);
    let queue = RecallQueue {
        wal: wal.clone(),
        wake: Arc::clone(&wake),
    };
    let join = tokio::spawn(run_recall_worker(
        use_case,
        wal,
        wake,
        config.drain_interval_ms,
        receiver,
    ));
    Ok((queue, RecallWorkerHandle { shutdown, join }))
}

async fn run_recall_worker(
    use_case: Arc<RecallUseCase>,
    wal: RecallWal,
    wake: Arc<Notify>,
    drain_interval_ms: u64,
    mut shutdown: watch::Receiver<bool>,
) {
    info!("encrypted recall spool Worker 已启动");
    loop {
        if *shutdown.borrow() {
            if let Err(error) = drain_wal_to_inbox(&wal, &use_case).await {
                warn!(error, "关闭前撤回 spool 转存失败；记录将由下次启动恢复");
            }
            return;
        }
        if let Err(error) = drain_wal_to_inbox(&wal, &use_case).await {
            warn!(error, "撤回 spool 转存 MySQL 失败，将保留记录重试");
        }
        match use_case.claim(RECALL_LEASE_SECS).await {
            Ok(Some(claimed)) => process_claim(&use_case, claimed).await,
            Ok(None) | Err(_) => {
                tokio::select! { _ = shutdown.changed() => {}, _ = wake.notified() => {}, _ = tokio::time::sleep(Duration::from_millis(drain_interval_ms)) => {} }
            }
        }
    }
}

async fn drain_wal_to_inbox(wal: &RecallWal, use_case: &RecallUseCase) -> Result<(), String> {
    let result = async {
        let snapshot = wal.snapshot()?;
        if snapshot.is_empty() {
            return Ok(());
        }
        for event in wal.events(&snapshot)? {
            use_case
                .enqueue(&event)
                .await
                .map_err(|error| error.to_string())?;
        }
        wal.remove_snapshot_prefix(&snapshot)
    }
    .await;
    if result.is_err() {
        wal.telemetry.mark_error(SPOOL_ERROR_DRAIN_FAILED, false);
    }
    result
}

async fn process_claim(use_case: &RecallUseCase, claimed: personal_secretary::ClaimedRecallEvent) {
    let event_id = claimed.event.recall_event_id.as_str();
    match use_case.handle_recall(&claimed.event).await {
        Ok(status) => {
            if let Err(error) = use_case.mark_applied(event_id, &claimed.lease_token).await {
                error!(recall_event_id = event_id, error = %error, "撤回已应用但 inbox ack 失败");
            } else {
                debug!(
                    recall_event_id = event_id,
                    status = status.as_str(),
                    "撤回 inbox 已完成"
                );
            }
        }
        Err(error) => {
            let (kind, code) = classify_failure(&error, claimed.attempt);
            if let Err(mark_error) = use_case
                .mark_failed(event_id, &claimed.lease_token, code, kind)
                .await
            {
                error!(recall_event_id = event_id, error = %error, mark_error = %mark_error, "撤回失败状态未能持久化");
            }
        }
    }
}
fn classify_failure(error: &RecallError, attempt: u32) -> (RecallFailureKind, &'static str) {
    match error {
        RecallError::InvalidIdentity(_) | RecallError::CorrelationCollision(_) => {
            (RecallFailureKind::Permanent, "invalid_recall")
        }
        RecallError::Store(_) if attempt >= MAX_RECALL_ATTEMPTS => {
            (RecallFailureKind::Permanent, "retry_exhausted")
        }
        RecallError::Store(_) => (RecallFailureKind::Retryable, "store_unavailable"),
    }
}

pub struct RecallHandler {
    queue: RecallQueue,
    account: SourceAccountRef,
    #[allow(dead_code)]
    self_qq_id: i64,
}
impl RecallHandler {
    pub fn new(queue: RecallQueue, account: SourceAccountRef, self_qq_id: i64) -> Self {
        Self {
            queue,
            account,
            self_qq_id,
        }
    }
    pub async fn handle_group_recall(&self, event: GroupRecallEvent) -> Result<(), NapCatError> {
        let conversation =
            ConversationRef::new(ConversationKind::Group, event.group_id.to_string())
                .map_err(|e| NapCatError::Protocol(format!("invalid conversation: {e}")))?;
        let correlation = RecallCorrelationKey::new(
            self.account.clone(),
            MessageSource::NapCat,
            conversation,
            event.message_id,
        )
        .map_err(|e| NapCatError::Protocol(format!("invalid correlation key: {e}")))?;
        self.queue
            .enqueue(RecallEvent {
                recall_event_id: RecallEventId::new(Uuid::new_v4().to_string())
                    .map_err(|e| NapCatError::Protocol(format!("invalid recall_event_id: {e}")))?,
                account: self.account.clone(),
                kind: RecallKind::Group,
                correlation,
                operator_platform_id: event.operator_id.map(|id| id.to_string()),
                occurred_at_unix_secs: event.time,
            })
            .await
    }
    pub async fn handle_friend_recall(&self, event: FriendRecallEvent) -> Result<(), NapCatError> {
        let conversation =
            ConversationRef::new(ConversationKind::Private, event.user_id.to_string())
                .map_err(|e| NapCatError::Protocol(format!("invalid conversation: {e}")))?;
        let correlation = RecallCorrelationKey::new(
            self.account.clone(),
            MessageSource::NapCat,
            conversation,
            event.message_id,
        )
        .map_err(|e| NapCatError::Protocol(format!("invalid correlation key: {e}")))?;
        self.queue
            .enqueue(RecallEvent {
                recall_event_id: RecallEventId::new(Uuid::new_v4().to_string())
                    .map_err(|e| NapCatError::Protocol(format!("invalid recall_event_id: {e}")))?,
                account: self.account.clone(),
                kind: RecallKind::Friend,
                correlation,
                operator_platform_id: Some(event.user_id.to_string()),
                occurred_at_unix_secs: event.time,
            })
            .await
    }
}
