//! Ordinary-message durable spool file adapter.
//!
//! This module owns a format, key, lock, checkpoint and lifecycle that are independent from the
//! Recall spool. It performs blocking file operations only; runtime scheduling is introduced by
//! GAP-007-IMPL-C.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use base64::Engine;
use fs2::FileExt;
use personal_secretary::{
    DurableSpoolReceipt, RealtimeSpoolAdmission, RealtimeSpoolAdmissionId,
    RealtimeSpoolCheckpointPrefix, RealtimeSpoolFatalKind, RealtimeSpoolGenerationId,
    RealtimeSpoolRecordId, RealtimeSpoolRecoveryFrame, RecoveredRealtimeSpoolFrame,
};
use ring::{
    aead, digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

const WAL_MAGIC: &[u8; 4] = b"QMWL";
const FRAME_MAGIC: &[u8; 4] = b"QMFR";
const CHECKPOINT_MAGIC: &[u8; 4] = b"QMCP";
const FORMAT_VERSION: u8 = 1;
const KEY_ID_LEN: usize = 16;
const UUID_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const WAL_HEADER_LEN: usize = 4 + 1 + KEY_ID_LEN + UUID_LEN + UUID_LEN;
const FRAME_HEADER_LEN: usize = 4 + 1 + KEY_ID_LEN + 4 + NONCE_LEN;
const CHECKPOINT_HEADER_LEN: usize = 4 + 1 + KEY_ID_LEN + 4 + NONCE_LEN;
const DEFAULT_MAX_FRAME_PLAINTEXT: usize = 1024 * 1024;
const ACTIVE_WAL_BYTES: u64 = 240 * 1024 * 1024;
const COMPACT_TEMP_BYTES: u64 = 240 * 1024 * 1024;
const QUARANTINE_BYTES: u64 = 16 * 1024 * 1024;
const METADATA_BYTES: u64 = 16 * 1024 * 1024;
const TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const ALLOCATION_GRANULARITY_RESERVE: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct RealtimeMessageSpoolConfig {
    pub wal_path: PathBuf,
    pub checkpoint_path: PathBuf,
    pub quarantine_dir: PathBuf,
    pub key_env: String,
    pub max_frame_plaintext: usize,
}

impl RealtimeMessageSpoolConfig {
    pub fn new(
        wal_path: PathBuf,
        checkpoint_path: PathBuf,
        quarantine_dir: PathBuf,
        key_env: impl Into<String>,
    ) -> Self {
        Self {
            wal_path,
            checkpoint_path,
            quarantine_dir,
            key_env: key_env.into(),
            max_frame_plaintext: DEFAULT_MAX_FRAME_PLAINTEXT,
        }
    }

    fn validate(&self) -> Result<(), RealtimeMessageSpoolError> {
        if self.wal_path.as_os_str().is_empty()
            || self.checkpoint_path.as_os_str().is_empty()
            || self.quarantine_dir.as_os_str().is_empty()
            || self.key_env.trim().is_empty()
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "config",
            ));
        }
        if self.wal_path == self.checkpoint_path
            || self.wal_path == self.quarantine_dir
            || self.checkpoint_path == self.quarantine_dir
            || self.max_frame_plaintext == 0
            || self.max_frame_plaintext > DEFAULT_MAX_FRAME_PLAINTEXT
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "config",
            ));
        }
        Ok(())
    }

    fn lock_path(&self) -> PathBuf {
        self.wal_path.with_extension("message-spool.lock")
    }

    fn compact_temp_path(&self) -> PathBuf {
        self.wal_path.with_extension("message-spool.compact.tmp")
    }

    fn checkpoint_temp_path(&self) -> PathBuf {
        self.checkpoint_path.with_extension("tmp")
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("realtime message spool failed at {stage}: {kind:?}")]
pub struct RealtimeMessageSpoolError {
    pub kind: RealtimeSpoolFatalKind,
    pub stage: &'static str,
}

fn spool_error(kind: RealtimeSpoolFatalKind, stage: &'static str) -> RealtimeMessageSpoolError {
    RealtimeMessageSpoolError { kind, stage }
}

pub struct RealtimeMessageSpoolOpen {
    pub spool: RealtimeMessageSpool,
    pub recovery: RealtimeMessageSpoolRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeMessageSpoolRecovery {
    pub frames: Vec<RealtimeSpoolRecoveryFrame>,
    pub truncated_final_tail: bool,
}

pub struct RealtimeMessageSpool {
    config: RealtimeMessageSpoolConfig,
    key: aead::LessSafeKey,
    key_id: [u8; KEY_ID_LEN],
    generation_uuid: Uuid,
    generation_id: RealtimeSpoolGenerationId,
    _process_lock: File,
    operation_lock: Mutex<()>,
    telemetry: Arc<RealtimeSpoolTelemetry>,
}

#[derive(Debug, Default)]
pub struct RealtimeSpoolTelemetry {
    observed: AtomicBool,
    usable: AtomicBool,
    bytes_used: AtomicU64,
    pending_frames: AtomicU64,
    oldest_occurred_at: AtomicU64,
    quarantine_count: AtomicU64,
    recent_error: AtomicU8,
    reconciliation_pending: AtomicBool,
}

#[derive(Debug, Clone, Copy)]
pub struct RealtimeSpoolTelemetrySnapshot {
    pub observed: bool,
    pub usable: bool,
    pub bytes_used: u64,
    pub capacity_bytes: u64,
    pub pending_frames: u64,
    pub oldest_occurred_at_unix_secs: Option<i64>,
    pub quarantine_count: u64,
    pub recent_error_code: Option<&'static str>,
    pub reconciliation_pending: bool,
}

impl RealtimeSpoolTelemetry {
    fn initialize(&self, bytes_used: u64, pending: &[&ScannedFrame], quarantine_count: u64) {
        self.observed.store(true, Ordering::Release);
        self.usable.store(true, Ordering::Release);
        self.bytes_used.store(bytes_used, Ordering::Release);
        self.pending_frames
            .store(pending.len() as u64, Ordering::Release);
        self.oldest_occurred_at.store(
            pending
                .first()
                .map(|frame| frame.recovered.message().occurred_at_unix_secs.max(0) as u64)
                .unwrap_or(0),
            Ordering::Release,
        );
        self.quarantine_count
            .store(quarantine_count, Ordering::Release);
    }

    fn record_append(&self, bytes_used: u64, occurred_at: i64) {
        let previous = self.pending_frames.fetch_add(1, Ordering::AcqRel);
        if previous == 0 {
            self.oldest_occurred_at
                .store(occurred_at.max(0) as u64, Ordering::Release);
        }
        self.bytes_used.store(bytes_used, Ordering::Release);
        self.usable.store(true, Ordering::Release);
        self.recent_error.store(0, Ordering::Release);
    }

    fn record_checkpoint(&self, bytes_used: u64, remaining: &[&ScannedFrame]) {
        self.bytes_used.store(bytes_used, Ordering::Release);
        self.pending_frames
            .store(remaining.len() as u64, Ordering::Release);
        self.oldest_occurred_at.store(
            remaining
                .first()
                .map(|frame| frame.recovered.message().occurred_at_unix_secs.max(0) as u64)
                .unwrap_or(0),
            Ordering::Release,
        );
        self.usable.store(true, Ordering::Release);
        self.recent_error.store(0, Ordering::Release);
    }

    pub fn mark_fatal(&self, kind: RealtimeSpoolFatalKind) {
        self.observed.store(true, Ordering::Release);
        self.usable.store(false, Ordering::Release);
        self.recent_error.store(fatal_code(kind), Ordering::Release);
    }

    pub fn set_reconciliation_pending(&self, pending: bool) {
        self.reconciliation_pending
            .store(pending, Ordering::Release);
    }

    pub fn snapshot(&self) -> RealtimeSpoolTelemetrySnapshot {
        let oldest = self.oldest_occurred_at.load(Ordering::Acquire);
        RealtimeSpoolTelemetrySnapshot {
            observed: self.observed.load(Ordering::Acquire),
            usable: self.usable.load(Ordering::Acquire),
            bytes_used: self.bytes_used.load(Ordering::Acquire),
            capacity_bytes: TOTAL_BYTES,
            pending_frames: self.pending_frames.load(Ordering::Acquire),
            oldest_occurred_at_unix_secs: (oldest > 0)
                .then_some(oldest.min(i64::MAX as u64) as i64),
            quarantine_count: self.quarantine_count.load(Ordering::Acquire),
            recent_error_code: fatal_code_name(self.recent_error.load(Ordering::Acquire)),
            reconciliation_pending: self.reconciliation_pending.load(Ordering::Acquire),
        }
    }
}

fn fatal_code(kind: RealtimeSpoolFatalKind) -> u8 {
    match kind {
        RealtimeSpoolFatalKind::KeyUnavailable => 1,
        RealtimeSpoolFatalKind::LockUnavailable => 2,
        RealtimeSpoolFatalKind::CapacityExhausted => 3,
        RealtimeSpoolFatalKind::AppendFailed => 4,
        RealtimeSpoolFatalKind::SyncFailed => 5,
        RealtimeSpoolFatalKind::WriterStopped => 6,
        RealtimeSpoolFatalKind::CheckpointFailed => 7,
        RealtimeSpoolFatalKind::RecoveryCorruptFrame => 8,
        RealtimeSpoolFatalKind::RecoveryInvariantViolation => 9,
        RealtimeSpoolFatalKind::ReconciliationFailed => 10,
    }
}

fn fatal_code_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => return None,
        1 => "key_unavailable",
        2 => "lock_unavailable",
        3 => "capacity_exhausted",
        4 => "append_failed",
        5 => "sync_failed",
        6 => "writer_stopped",
        7 => "checkpoint_failed",
        8 => "recovery_corrupt_frame",
        9 => "recovery_invariant_violation",
        10 => "reconciliation_failed",
        _ => "unknown_spool_error",
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredMessageFrame {
    admission_id: RealtimeSpoolAdmissionId,
    record_id: RealtimeSpoolRecordId,
    connection_epoch_id: personal_secretary::ConnectionEpochId,
    message: personal_secretary::InboundMessageEnvelope,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredCheckpoint {
    generation_id: RealtimeSpoolGenerationId,
    last_record_id: Option<RealtimeSpoolRecordId>,
}

struct ScannedFrame {
    stored: StoredMessageFrame,
    recovered: RecoveredRealtimeSpoolFrame,
    offset: u64,
    encoded_len: u64,
}

struct ScannedWal {
    base_record_id: Option<RealtimeSpoolRecordId>,
    frames: Vec<ScannedFrame>,
    truncated_final_tail: bool,
}

impl RealtimeMessageSpool {
    pub fn open(
        config: RealtimeMessageSpoolConfig,
    ) -> Result<RealtimeMessageSpoolOpen, RealtimeMessageSpoolError> {
        config.validate()?;
        create_parent(&config.wal_path)?;
        create_parent(&config.checkpoint_path)?;
        std::fs::create_dir_all(&config.quarantine_dir).map_err(|_| {
            spool_error(RealtimeSpoolFatalKind::RecoveryInvariantViolation, "mkdir")
        })?;

        let (key, key_id) = load_key(&config.key_env)?;
        let lock_path = config.lock_path();
        let process_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::LockUnavailable, "lock_open"))?;
        process_lock
            .try_lock_exclusive()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::LockUnavailable, "lock_acquire"))?;

        remove_non_authoritative_temp(&config.compact_temp_path())?;
        remove_non_authoritative_temp(&config.checkpoint_temp_path())?;

        let (generation_uuid, generation_id) = initialize_or_read_wal(&config, &key_id)?;
        let telemetry = Arc::new(RealtimeSpoolTelemetry::default());
        let spool = Self {
            config,
            key,
            key_id,
            generation_uuid,
            generation_id,
            _process_lock: process_lock,
            operation_lock: Mutex::new(()),
            telemetry: Arc::clone(&telemetry),
        };
        let _guard = spool
            .operation_lock
            .lock()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::WriterStopped, "operation_lock"))?;
        let scan = spool.scan_locked(true)?;
        let checkpoint = spool.read_checkpoint_locked()?;
        let pending = spool.pending_after_checkpoint(&scan, checkpoint.as_ref())?;
        spool.verify_budget_locked()?;
        telemetry.initialize(
            spool.disk_usage_locked()?,
            &pending,
            directory_file_count(&spool.config.quarantine_dir)?,
        );
        let mut frames = pending
            .into_iter()
            .map(|frame| RealtimeSpoolRecoveryFrame::Replay(Box::new(frame.recovered.clone())))
            .collect::<Vec<_>>();
        if scan.truncated_final_tail {
            frames.push(RealtimeSpoolRecoveryFrame::TruncateIncompleteFinalTail);
        }
        let truncated_final_tail = scan.truncated_final_tail;
        drop(_guard);
        Ok(RealtimeMessageSpoolOpen {
            spool,
            recovery: RealtimeMessageSpoolRecovery {
                frames,
                truncated_final_tail,
            },
        })
    }

    pub fn generation_id(&self) -> &RealtimeSpoolGenerationId {
        &self.generation_id
    }

    pub fn telemetry(&self) -> Arc<RealtimeSpoolTelemetry> {
        Arc::clone(&self.telemetry)
    }

    pub fn append(
        &self,
        admission: &RealtimeSpoolAdmission,
    ) -> Result<DurableSpoolReceipt, RealtimeMessageSpoolError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::WriterStopped, "operation_lock"))?;
        let record_uuid = Uuid::new_v4();
        let record_id = RealtimeSpoolRecordId::new(record_uuid.to_string()).map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "record_id",
            )
        })?;
        let stored = StoredMessageFrame {
            admission_id: admission.admission_id().clone(),
            record_id: record_id.clone(),
            connection_epoch_id: admission.connection_epoch_id().clone(),
            message: admission.message().clone(),
        };
        let plaintext = serde_json::to_vec(&stored)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::AppendFailed, "serialize"))?;
        if plaintext.len() > self.config.max_frame_plaintext {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "frame_limit",
            ));
        }
        let frame = self.encrypt_frame(&plaintext)?;
        self.ensure_append_budget_locked(frame.len() as u64)?;
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.config.wal_path)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::AppendFailed, "append_open"))?;
        file.write_all(&frame)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::AppendFailed, "append_write"))?;
        file.sync_all()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "append_sync"))?;
        self.telemetry.record_append(
            self.disk_usage_locked()?,
            admission.message().occurred_at_unix_secs,
        );
        Ok(DurableSpoolReceipt::new(
            admission.admission_id().clone(),
            self.generation_id.clone(),
            record_id,
        ))
    }

    pub fn recover_pending(
        &self,
    ) -> Result<Vec<RecoveredRealtimeSpoolFrame>, RealtimeMessageSpoolError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::WriterStopped, "operation_lock"))?;
        let scan = self.scan_locked(false)?;
        let checkpoint = self.read_checkpoint_locked()?;
        Ok(self
            .pending_after_checkpoint(&scan, checkpoint.as_ref())?
            .into_iter()
            .map(|frame| frame.recovered.clone())
            .collect())
    }

    pub fn advance_checkpoint(
        &self,
        prefix: &RealtimeSpoolCheckpointPrefix,
    ) -> Result<(), RealtimeMessageSpoolError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::WriterStopped, "operation_lock"))?;
        if prefix.generation_id() != &self.generation_id {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CheckpointFailed,
                "checkpoint_generation",
            ));
        }
        if prefix.record_ids().is_empty() {
            return Ok(());
        }
        let scan = self.scan_locked(false)?;
        let checkpoint = self.read_checkpoint_locked()?;
        let pending = self.pending_after_checkpoint(&scan, checkpoint.as_ref())?;
        if prefix.record_ids().len() > pending.len()
            || prefix
                .record_ids()
                .iter()
                .zip(&pending)
                .any(|(expected, actual)| expected != &actual.stored.record_id)
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CheckpointFailed,
                "checkpoint_prefix",
            ));
        }
        let checkpoint = StoredCheckpoint {
            generation_id: self.generation_id.clone(),
            last_record_id: prefix.record_ids().last().cloned(),
        };
        self.write_checkpoint_locked(&checkpoint)?;
        self.telemetry.record_checkpoint(
            self.disk_usage_locked()?,
            &pending[prefix.record_ids().len()..],
        );
        Ok(())
    }

    pub fn compact(&self) -> Result<(), RealtimeMessageSpoolError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::WriterStopped, "operation_lock"))?;
        let scan = self.scan_locked(false)?;
        let Some(checkpoint) = self.read_checkpoint_locked()? else {
            return Ok(());
        };
        let Some(last_record_id) = checkpoint.last_record_id.as_ref() else {
            return Ok(());
        };
        let pending = self.pending_after_checkpoint(&scan, Some(&checkpoint))?;
        let compact_len =
            WAL_HEADER_LEN as u64 + pending.iter().map(|frame| frame.encoded_len).sum::<u64>();
        if compact_len > COMPACT_TEMP_BYTES {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "compact_partition",
            ));
        }
        self.ensure_compact_budget_locked(compact_len)?;

        let temp_path = self.config.compact_temp_path();
        let mut temp = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "compact_open"))?;
        write_wal_header(
            &mut temp,
            &self.key_id,
            self.generation_uuid,
            Some(record_uuid(last_record_id)?),
        )?;
        let mut source = OpenOptions::new()
            .read(true)
            .open(&self.config.wal_path)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "compact_source"))?;
        let mut buffer = vec![0_u8; 64 * 1024];
        for frame in pending {
            source.seek(SeekFrom::Start(frame.offset)).map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "compact_seek")
            })?;
            copy_exact(&mut source, &mut temp, frame.encoded_len, &mut buffer)?;
        }
        temp.sync_all()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "compact_sync"))?;
        drop(source);
        drop(temp);
        durable_replace(&temp_path, &self.config.wal_path)?;
        self.scan_locked(false)?;
        self.verify_budget_locked()
    }

    fn encrypt_frame(&self, plaintext: &[u8]) -> Result<Vec<u8>, RealtimeMessageSpoolError> {
        let nonce_bytes = random_nonce()?;
        let encrypted_len = plaintext
            .len()
            .checked_add(TAG_LEN)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                spool_error(RealtimeSpoolFatalKind::CapacityExhausted, "frame_length")
            })?;
        let header = frame_header(&self.key_id, encrypted_len, &nonce_bytes);
        let aad = frame_aad(&header, self.generation_uuid);
        let mut encrypted = plaintext.to_vec();
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce_bytes),
                aead::Aad::from(aad.as_slice()),
                &mut encrypted,
            )
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::AppendFailed, "frame_encrypt"))?;
        let mut frame = header;
        frame.extend_from_slice(&encrypted);
        Ok(frame)
    }

    fn scan_locked(&self, repair_tail: bool) -> Result<ScannedWal, RealtimeMessageSpoolError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(repair_tail)
            .open(&self.config.wal_path)
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "wal_open",
                )
            })?;
        let file_len = file
            .metadata()
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "wal_metadata",
                )
            })?
            .len();
        if file_len < WAL_HEADER_LEN as u64 {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryCorruptFrame,
                "wal_header",
            ));
        }
        let mut wal_header = [0_u8; WAL_HEADER_LEN];
        file.read_exact(&mut wal_header)
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "wal_header"))?;
        let (generation_uuid, base_record_id) = parse_wal_header(&wal_header, &self.key_id)?;
        if generation_uuid != self.generation_uuid {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "wal_generation",
            ));
        }

        let mut offset = WAL_HEADER_LEN as u64;
        let mut frames = Vec::new();
        let mut record_ids = HashSet::new();
        let mut admission_ids = HashSet::new();
        let mut truncated_final_tail = false;
        while offset < file_len {
            let remaining = file_len - offset;
            if remaining < FRAME_HEADER_LEN as u64 {
                truncate_final_tail(&mut file, offset, repair_tail)?;
                truncated_final_tail = true;
                break;
            }
            file.seek(SeekFrom::Start(offset)).map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "frame_seek")
            })?;
            let mut header = [0_u8; FRAME_HEADER_LEN];
            file.read_exact(&mut header).map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "frame_header")
            })?;
            let (encrypted_len, nonce) =
                parse_frame_header(&header, &self.key_id, self.config.max_frame_plaintext)?;
            let encoded_len = FRAME_HEADER_LEN as u64 + encrypted_len as u64;
            if remaining < encoded_len {
                truncate_final_tail(&mut file, offset, repair_tail)?;
                truncated_final_tail = true;
                break;
            }
            let mut encrypted = vec![0_u8; encrypted_len];
            file.read_exact(&mut encrypted).map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "frame_read")
            })?;
            let aad = frame_aad(&header, self.generation_uuid);
            let plaintext = self
                .key
                .open_in_place(
                    aead::Nonce::assume_unique_for_key(nonce),
                    aead::Aad::from(aad.as_slice()),
                    &mut encrypted,
                )
                .map_err(|_| {
                    spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "frame_auth")
                })?;
            let stored: StoredMessageFrame = serde_json::from_slice(plaintext).map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "frame_decode")
            })?;
            RealtimeSpoolAdmissionId::new(stored.admission_id.as_str()).map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "admission_id",
                )
            })?;
            record_uuid(&stored.record_id)?;
            if !record_ids.insert(stored.record_id.clone()) {
                return Err(spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "duplicate_record",
                ));
            }
            if !admission_ids.insert(stored.admission_id.clone()) {
                return Err(spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "duplicate_admission",
                ));
            }
            let recovered = RecoveredRealtimeSpoolFrame::new(
                self.generation_id.clone(),
                stored.record_id.clone(),
                stored.connection_epoch_id.clone(),
                stored.message.clone(),
            )
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "frame_identity",
                )
            })?;
            frames.push(ScannedFrame {
                stored,
                recovered,
                offset,
                encoded_len,
            });
            offset += encoded_len;
        }
        Ok(ScannedWal {
            base_record_id,
            frames,
            truncated_final_tail,
        })
    }

    fn pending_after_checkpoint<'a>(
        &self,
        scan: &'a ScannedWal,
        checkpoint: Option<&StoredCheckpoint>,
    ) -> Result<Vec<&'a ScannedFrame>, RealtimeMessageSpoolError> {
        let Some(checkpoint) = checkpoint else {
            if scan.base_record_id.is_some() {
                return Err(spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "checkpoint_missing",
                ));
            }
            return Ok(scan.frames.iter().collect());
        };
        if checkpoint.generation_id != self.generation_id {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "checkpoint_generation",
            ));
        }
        let Some(last_record_id) = checkpoint.last_record_id.as_ref() else {
            if scan.base_record_id.is_some() {
                return Err(spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "checkpoint_base",
                ));
            }
            return Ok(scan.frames.iter().collect());
        };
        if scan.base_record_id.as_ref() == Some(last_record_id) {
            return Ok(scan.frames.iter().collect());
        }
        let Some(index) = scan
            .frames
            .iter()
            .position(|frame| &frame.stored.record_id == last_record_id)
        else {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "checkpoint_record",
            ));
        };
        Ok(scan.frames[index + 1..].iter().collect())
    }

    fn read_checkpoint_locked(
        &self,
    ) -> Result<Option<StoredCheckpoint>, RealtimeMessageSpoolError> {
        if !self.config.checkpoint_path.exists() {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&self.config.checkpoint_path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "checkpoint_read",
                )
            })?;
        if bytes.len() < CHECKPOINT_HEADER_LEN + TAG_LEN {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "checkpoint_format",
            ));
        }
        let header = &bytes[..CHECKPOINT_HEADER_LEN];
        let (encrypted_len, nonce) = parse_checkpoint_header(header, &self.key_id)?;
        if bytes.len() != CHECKPOINT_HEADER_LEN + encrypted_len {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "checkpoint_length",
            ));
        }
        let mut encrypted = bytes[CHECKPOINT_HEADER_LEN..].to_vec();
        let plaintext = self
            .key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(header),
                &mut encrypted,
            )
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "checkpoint_auth",
                )
            })?;
        let checkpoint: StoredCheckpoint = serde_json::from_slice(plaintext).map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "checkpoint_decode",
            )
        })?;
        if let Some(record_id) = checkpoint.last_record_id.as_ref() {
            record_uuid(record_id)?;
        }
        Ok(Some(checkpoint))
    }

    fn write_checkpoint_locked(
        &self,
        checkpoint: &StoredCheckpoint,
    ) -> Result<(), RealtimeMessageSpoolError> {
        let plaintext = serde_json::to_vec(checkpoint).map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::CheckpointFailed,
                "checkpoint_serialize",
            )
        })?;
        let nonce_bytes = random_nonce()?;
        let encrypted_len = plaintext.len().checked_add(TAG_LEN).ok_or_else(|| {
            spool_error(
                RealtimeSpoolFatalKind::CheckpointFailed,
                "checkpoint_length",
            )
        })?;
        let encrypted_len_u32 = u32::try_from(encrypted_len).map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::CheckpointFailed,
                "checkpoint_length",
            )
        })?;
        let header = checkpoint_header(&self.key_id, encrypted_len_u32, &nonce_bytes);
        let mut encrypted = plaintext;
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce_bytes),
                aead::Aad::from(header.as_slice()),
                &mut encrypted,
            )
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::CheckpointFailed,
                    "checkpoint_encrypt",
                )
            })?;
        let total_len = (header.len() + encrypted.len()) as u64;
        let reserved_len = total_len.saturating_add(ALLOCATION_GRANULARITY_RESERVE);
        if reserved_len > METADATA_BYTES
            || self.metadata_bytes_locked()?.saturating_add(reserved_len) > METADATA_BYTES
            || self.disk_usage_locked()?.saturating_add(reserved_len) > TOTAL_BYTES
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "metadata_partition",
            ));
        }
        let temp_path = self.config.checkpoint_temp_path();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "checkpoint_open")
            })?;
        file.write_all(&header)
            .and_then(|_| file.write_all(&encrypted))
            .map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "checkpoint_write")
            })?;
        file.sync_all()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "checkpoint_sync"))?;
        drop(file);
        durable_replace(&temp_path, &self.config.checkpoint_path)?;
        self.verify_budget_locked()
    }

    fn ensure_append_budget_locked(
        &self,
        additional: u64,
    ) -> Result<(), RealtimeMessageSpoolError> {
        let reserved = additional.saturating_add(ALLOCATION_GRANULARITY_RESERVE);
        let wal_logical = file_len(&self.config.wal_path)?;
        let wal_allocated = allocated_file_bytes(&self.config.wal_path)?;
        if wal_logical.saturating_add(additional) > ACTIVE_WAL_BYTES
            || wal_allocated.saturating_add(reserved) > ACTIVE_WAL_BYTES
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "active_wal_partition",
            ));
        }
        let usage = self.disk_usage_locked()?;
        if usage.saturating_add(reserved) > TOTAL_BYTES {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "total_budget",
            ));
        }
        Ok(())
    }

    fn ensure_compact_budget_locked(
        &self,
        compact_len: u64,
    ) -> Result<(), RealtimeMessageSpoolError> {
        let usage = self.disk_usage_locked()?;
        if usage
            .saturating_add(compact_len)
            .saturating_add(ALLOCATION_GRANULARITY_RESERVE)
            > TOTAL_BYTES
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "compact_peak_budget",
            ));
        }
        Ok(())
    }

    fn verify_budget_locked(&self) -> Result<(), RealtimeMessageSpoolError> {
        if file_len(&self.config.wal_path)? > ACTIVE_WAL_BYTES
            || allocated_file_bytes(&self.config.wal_path)? > ACTIVE_WAL_BYTES
            || file_len(&self.config.compact_temp_path())? > COMPACT_TEMP_BYTES
            || allocated_file_bytes(&self.config.compact_temp_path())? > COMPACT_TEMP_BYTES
            || directory_file_bytes(&self.config.quarantine_dir)? > QUARANTINE_BYTES
            || self.metadata_bytes_locked()? > METADATA_BYTES
            || self.disk_usage_locked()? > TOTAL_BYTES
        {
            return Err(spool_error(
                RealtimeSpoolFatalKind::CapacityExhausted,
                "budget_verify",
            ));
        }
        Ok(())
    }

    fn metadata_bytes_locked(&self) -> Result<u64, RealtimeMessageSpoolError> {
        Ok(allocated_file_bytes(&self.config.checkpoint_path)?
            .saturating_add(allocated_file_bytes(&self.config.checkpoint_temp_path())?)
            .saturating_add(allocated_file_bytes(&self.config.lock_path())?))
    }

    fn disk_usage_locked(&self) -> Result<u64, RealtimeMessageSpoolError> {
        Ok(allocated_file_bytes(&self.config.wal_path)?
            .saturating_add(allocated_file_bytes(&self.config.compact_temp_path())?)
            .saturating_add(directory_file_bytes(&self.config.quarantine_dir)?)
            .saturating_add(self.metadata_bytes_locked()?))
    }
}

fn load_key(
    env_name: &str,
) -> Result<(aead::LessSafeKey, [u8; KEY_ID_LEN]), RealtimeMessageSpoolError> {
    let encoded = std::env::var(env_name)
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::KeyUnavailable, "key_load"))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::KeyUnavailable, "key_decode"))?;
    if raw.len() != 32 {
        return Err(spool_error(
            RealtimeSpoolFatalKind::KeyUnavailable,
            "key_length",
        ));
    }
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &raw)
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::KeyUnavailable, "key_init"))?;
    let hash = digest::digest(&digest::SHA256, &raw);
    let mut key_id = [0_u8; KEY_ID_LEN];
    key_id.copy_from_slice(&hash.as_ref()[..KEY_ID_LEN]);
    Ok((aead::LessSafeKey::new(unbound), key_id))
}

fn initialize_or_read_wal(
    config: &RealtimeMessageSpoolConfig,
    key_id: &[u8; KEY_ID_LEN],
) -> Result<(Uuid, RealtimeSpoolGenerationId), RealtimeMessageSpoolError> {
    if !config.wal_path.exists() {
        let generation = Uuid::new_v4();
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&config.wal_path)
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "wal_create",
                )
            })?;
        write_wal_header(&mut file, key_id, generation, None)?;
        file.sync_all()
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "wal_header_sync"))?;
        sync_parent(&config.wal_path)?;
        let generation_id =
            RealtimeSpoolGenerationId::new(generation.to_string()).map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "generation_id",
                )
            })?;
        return Ok((generation, generation_id));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(&config.wal_path)
        .map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "wal_open",
            )
        })?;
    let mut header = [0_u8; WAL_HEADER_LEN];
    file.read_exact(&mut header)
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "wal_header"))?;
    let (generation, _) = parse_wal_header(&header, key_id)?;
    let generation_id = RealtimeSpoolGenerationId::new(generation.to_string()).map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "generation_id",
        )
    })?;
    Ok((generation, generation_id))
}

fn write_wal_header(
    file: &mut File,
    key_id: &[u8; KEY_ID_LEN],
    generation: Uuid,
    base_record: Option<Uuid>,
) -> Result<(), RealtimeMessageSpoolError> {
    file.write_all(WAL_MAGIC)
        .and_then(|_| file.write_all(&[FORMAT_VERSION]))
        .and_then(|_| file.write_all(key_id))
        .and_then(|_| file.write_all(generation.as_bytes()))
        .and_then(|_| file.write_all(base_record.unwrap_or(Uuid::nil()).as_bytes()))
        .map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "wal_header_write",
            )
        })
}

fn parse_wal_header(
    header: &[u8; WAL_HEADER_LEN],
    key_id: &[u8; KEY_ID_LEN],
) -> Result<(Uuid, Option<RealtimeSpoolRecordId>), RealtimeMessageSpoolError> {
    if &header[..4] != WAL_MAGIC || header[4] != FORMAT_VERSION {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "wal_header",
        ));
    }
    if &header[5..5 + KEY_ID_LEN] != key_id {
        return Err(spool_error(
            RealtimeSpoolFatalKind::KeyUnavailable,
            "wal_key_id",
        ));
    }
    let generation = Uuid::from_slice(&header[21..37]).map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "wal_generation",
        )
    })?;
    if generation.is_nil() {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "wal_generation",
        ));
    }
    let base = Uuid::from_slice(&header[37..53])
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "wal_base"))?;
    let base_record_id =
        if base.is_nil() {
            None
        } else {
            Some(RealtimeSpoolRecordId::new(base.to_string()).map_err(|_| {
                spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "wal_base")
            })?)
        };
    Ok((generation, base_record_id))
}

fn frame_header(key_id: &[u8; KEY_ID_LEN], encrypted_len: u32, nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut header = Vec::with_capacity(FRAME_HEADER_LEN);
    header.extend_from_slice(FRAME_MAGIC);
    header.push(FORMAT_VERSION);
    header.extend_from_slice(key_id);
    header.extend_from_slice(&encrypted_len.to_be_bytes());
    header.extend_from_slice(nonce);
    header
}

fn frame_aad(header: &[u8], generation: Uuid) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + UUID_LEN);
    aad.extend_from_slice(header);
    aad.extend_from_slice(generation.as_bytes());
    aad
}

fn parse_frame_header(
    header: &[u8; FRAME_HEADER_LEN],
    key_id: &[u8; KEY_ID_LEN],
    max_plaintext: usize,
) -> Result<(usize, [u8; NONCE_LEN]), RealtimeMessageSpoolError> {
    if &header[..4] != FRAME_MAGIC || header[4] != FORMAT_VERSION {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "frame_header",
        ));
    }
    if &header[5..21] != key_id {
        return Err(spool_error(
            RealtimeSpoolFatalKind::KeyUnavailable,
            "frame_key_id",
        ));
    }
    let encrypted_len =
        u32::from_be_bytes(header[21..25].try_into().map_err(|_| {
            spool_error(RealtimeSpoolFatalKind::RecoveryCorruptFrame, "frame_length")
        })?) as usize;
    if encrypted_len < TAG_LEN || encrypted_len > max_plaintext.saturating_add(TAG_LEN) {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "frame_length",
        ));
    }
    let mut nonce = [0_u8; NONCE_LEN];
    nonce.copy_from_slice(&header[25..37]);
    Ok((encrypted_len, nonce))
}

fn checkpoint_header(
    key_id: &[u8; KEY_ID_LEN],
    encrypted_len: u32,
    nonce: &[u8; NONCE_LEN],
) -> Vec<u8> {
    let mut header = Vec::with_capacity(CHECKPOINT_HEADER_LEN);
    header.extend_from_slice(CHECKPOINT_MAGIC);
    header.push(FORMAT_VERSION);
    header.extend_from_slice(key_id);
    header.extend_from_slice(&encrypted_len.to_be_bytes());
    header.extend_from_slice(nonce);
    header
}

fn parse_checkpoint_header(
    header: &[u8],
    key_id: &[u8; KEY_ID_LEN],
) -> Result<(usize, [u8; NONCE_LEN]), RealtimeMessageSpoolError> {
    if header.len() != CHECKPOINT_HEADER_LEN
        || &header[..4] != CHECKPOINT_MAGIC
        || header[4] != FORMAT_VERSION
    {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "checkpoint_header",
        ));
    }
    if &header[5..21] != key_id {
        return Err(spool_error(
            RealtimeSpoolFatalKind::KeyUnavailable,
            "checkpoint_key_id",
        ));
    }
    let encrypted_len = u32::from_be_bytes(header[21..25].try_into().map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "checkpoint_length",
        )
    })?) as usize;
    if encrypted_len < TAG_LEN || encrypted_len > METADATA_BYTES as usize {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "checkpoint_length",
        ));
    }
    let mut nonce = [0_u8; NONCE_LEN];
    nonce.copy_from_slice(&header[25..37]);
    Ok((encrypted_len, nonce))
}

fn random_nonce() -> Result<[u8; NONCE_LEN], RealtimeMessageSpoolError> {
    let mut nonce = [0_u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::AppendFailed, "random_nonce"))?;
    Ok(nonce)
}

fn record_uuid(record_id: &RealtimeSpoolRecordId) -> Result<Uuid, RealtimeMessageSpoolError> {
    Uuid::parse_str(record_id.as_str()).map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "record_uuid",
        )
    })
}

fn truncate_final_tail(
    file: &mut File,
    offset: u64,
    repair_tail: bool,
) -> Result<(), RealtimeMessageSpoolError> {
    if !repair_tail {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "incomplete_tail",
        ));
    }
    file.set_len(offset).map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryCorruptFrame,
            "tail_truncate",
        )
    })?;
    file.sync_all()
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "tail_sync"))
}

fn copy_exact(
    source: &mut File,
    target: &mut File,
    mut remaining: u64,
    buffer: &mut [u8],
) -> Result<(), RealtimeMessageSpoolError> {
    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        source
            .read_exact(&mut buffer[..chunk])
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "compact_read"))?;
        target
            .write_all(&buffer[..chunk])
            .map_err(|_| spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "compact_write"))?;
        remaining -= chunk as u64;
    }
    Ok(())
}

fn create_parent(path: &Path) -> Result<(), RealtimeMessageSpoolError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "parent_create",
            )
        })?;
    }
    Ok(())
}

fn remove_non_authoritative_temp(path: &Path) -> Result<(), RealtimeMessageSpoolError> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "temp_cleanup",
            )
        })?;
        sync_parent(path)?;
    }
    Ok(())
}

fn file_len(path: &Path) -> Result<u64, RealtimeMessageSpoolError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
        Ok(_) => Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "path_type",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "metadata",
        )),
    }
}

fn directory_file_bytes(path: &Path) -> Result<u64, RealtimeMessageSpoolError> {
    let mut total = 0_u64;
    let entries = std::fs::read_dir(path).map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "quarantine_read",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "quarantine_entry",
            )
        })?;
        let metadata = entry.metadata().map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "quarantine_metadata",
            )
        })?;
        if !metadata.is_file() {
            return Err(spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "quarantine_type",
            ));
        }
        total = total.saturating_add(allocated_file_bytes(&entry.path())?);
    }
    Ok(total)
}

fn directory_file_count(path: &Path) -> Result<u64, RealtimeMessageSpoolError> {
    if !path.exists() {
        return Ok(0);
    }
    let mut count = 0_u64;
    for entry in std::fs::read_dir(path).map_err(|_| {
        spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "dir_read",
        )
    })? {
        let entry = entry.map_err(|_| {
            spool_error(
                RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                "dir_entry",
            )
        })?;
        if entry
            .file_type()
            .map_err(|_| {
                spool_error(
                    RealtimeSpoolFatalKind::RecoveryInvariantViolation,
                    "dir_type",
                )
            })?
            .is_file()
        {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

#[cfg(windows)]
fn allocated_file_bytes(path: &Path) -> Result<u64, RealtimeMessageSpoolError> {
    use std::os::windows::ffi::OsStrExt;

    const INVALID_FILE_SIZE: u32 = u32::MAX;
    unsafe extern "system" {
        fn GetCompressedFileSizeW(file_name: *const u16, high: *mut u32) -> u32;
        fn GetLastError() -> u32;
    }
    if !path.exists() {
        return Ok(0);
    }
    let encoded = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut high = 0_u32;
    // Reports physical allocation and therefore includes filesystem cluster granularity.
    let low = unsafe { GetCompressedFileSizeW(encoded.as_ptr(), &mut high) };
    if low == INVALID_FILE_SIZE && unsafe { GetLastError() } != 0 {
        return Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "allocated_size",
        ));
    }
    Ok((u64::from(high) << 32) | u64::from(low))
}

#[cfg(unix)]
fn allocated_file_bytes(path: &Path) -> Result<u64, RealtimeMessageSpoolError> {
    use std::os::unix::fs::MetadataExt;

    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(metadata.blocks().saturating_mul(512)),
        Ok(_) => Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "path_type",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err(spool_error(
            RealtimeSpoolFatalKind::RecoveryInvariantViolation,
            "allocated_size",
        )),
    }
}

#[cfg(not(any(windows, unix)))]
fn allocated_file_bytes(path: &Path) -> Result<u64, RealtimeMessageSpoolError> {
    file_len(path)
}

fn durable_replace(source: &Path, target: &Path) -> Result<(), RealtimeMessageSpoolError> {
    durable_replace_platform(source, target)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(target)
        .and_then(|file| file.sync_all())
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "replace_sync"))?;
    sync_parent(target)
}

#[cfg(not(windows))]
fn durable_replace_platform(source: &Path, target: &Path) -> Result<(), RealtimeMessageSpoolError> {
    std::fs::rename(source, target)
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::CheckpointFailed, "replace"))
}

#[cfg(windows)]
fn durable_replace_platform(source: &Path, target: &Path) -> Result<(), RealtimeMessageSpoolError> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let existing = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let new = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // MOVEFILE_WRITE_THROUGH is the Windows durability primitive for the metadata replacement.
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(spool_error(
            RealtimeSpoolFatalKind::CheckpointFailed,
            "replace",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent(path: &Path) -> Result<(), RealtimeMessageSpoolError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| spool_error(RealtimeSpoolFatalKind::SyncFailed, "parent_sync"))
}

#[cfg(windows)]
fn sync_parent(_path: &Path) -> Result<(), RealtimeMessageSpoolError> {
    // File creation is synced through the file handle; replacements use MOVEFILE_WRITE_THROUGH.
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom};

    use personal_secretary::{
        ConversationKind, ConversationRef, InboundMessageEnvelope, MessageSource,
        RealtimeSpoolReplayProgress, SourceMessageRef, VerifiedActor, VerifiedActorKind,
        checkpointable_prefix,
    };
    use tempfile::TempDir;

    use super::*;

    fn install_key(name: &str, byte: u8) {
        // Tests use process-unique environment variable names and never overwrite production keys.
        unsafe {
            std::env::set_var(
                name,
                base64::engine::general_purpose::STANDARD.encode([byte; 32]),
            );
        }
    }

    fn config(temp: &TempDir, key_env: &str) -> RealtimeMessageSpoolConfig {
        RealtimeMessageSpoolConfig::new(
            temp.path().join("message.wal"),
            temp.path().join("message.checkpoint"),
            temp.path().join("message-quarantine"),
            key_env,
        )
    }

    fn admission(id: &str, text: &str) -> RealtimeSpoolAdmission {
        let epoch = personal_secretary::ConnectionEpochId::new("epoch-1").unwrap();
        let message = InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, "account-1", id).unwrap(),
            ConversationRef::new(ConversationKind::Group, "conversation-1").unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "actor-1").unwrap(),
            100,
            text,
            Vec::new(),
        )
        .unwrap()
        .observed_in(epoch.clone());
        RealtimeSpoolAdmission::new(
            RealtimeSpoolAdmissionId::new(format!("admission-{id}")).unwrap(),
            epoch,
            message,
        )
        .unwrap()
    }

    #[test]
    fn append_is_encrypted_and_recovered_without_runtime_receipt() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_1", 1);
        let opened =
            RealtimeMessageSpool::open(config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_1")).unwrap();
        let receipt = opened
            .spool
            .append(&admission("message-1", "private plaintext marker"))
            .unwrap();
        let wal = std::fs::read(temp.path().join("message.wal")).unwrap();

        assert!(
            !wal.windows(b"private plaintext marker".len())
                .any(|value| value == b"private plaintext marker")
        );
        assert_eq!(receipt.generation_id, opened.spool.generation_id().clone());
        assert_eq!(opened.spool.recover_pending().unwrap().len(), 1);
    }

    #[test]
    fn wrong_key_fails_closed_without_overwriting_wal() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_2A", 2);
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_2B", 3);
        let config_a = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_2A");
        let opened = RealtimeMessageSpool::open(config_a.clone()).unwrap();
        opened
            .spool
            .append(&admission("message-1", "secret"))
            .unwrap();
        let before = std::fs::read(&config_a.wal_path).unwrap();
        drop(opened);
        let error = RealtimeMessageSpool::open(config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_2B"))
            .err()
            .unwrap();

        assert_eq!(error.kind, RealtimeSpoolFatalKind::KeyUnavailable);
        assert_eq!(std::fs::read(&config_a.wal_path).unwrap(), before);
    }

    #[test]
    fn only_final_incomplete_tail_is_truncated() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_3", 4);
        let config = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_3");
        let opened = RealtimeMessageSpool::open(config.clone()).unwrap();
        opened.spool.append(&admission("message-1", "one")).unwrap();
        drop(opened);
        let verified_len = std::fs::metadata(&config.wal_path).unwrap().len();
        OpenOptions::new()
            .append(true)
            .open(&config.wal_path)
            .unwrap()
            .write_all(b"partial")
            .unwrap();

        let reopened = RealtimeMessageSpool::open(config.clone()).unwrap();

        assert!(reopened.recovery.truncated_final_tail);
        assert_eq!(
            std::fs::metadata(&config.wal_path).unwrap().len(),
            verified_len
        );
        assert_eq!(reopened.spool.recover_pending().unwrap().len(), 1);
    }

    #[test]
    fn complete_frame_authentication_failure_preserves_evidence() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_4", 5);
        let config = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_4");
        let opened = RealtimeMessageSpool::open(config.clone()).unwrap();
        opened.spool.append(&admission("message-1", "one")).unwrap();
        drop(opened);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&config.wal_path)
            .unwrap();
        let corrupt_offset = WAL_HEADER_LEN as u64 + FRAME_HEADER_LEN as u64;
        file.seek(SeekFrom::Start(corrupt_offset)).unwrap();
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 0x80;
        file.seek(SeekFrom::Start(corrupt_offset)).unwrap();
        file.write_all(&byte).unwrap();
        file.sync_all().unwrap();
        let before = std::fs::read(&config.wal_path).unwrap();

        let error = RealtimeMessageSpool::open(config.clone()).err().unwrap();

        assert_eq!(error.kind, RealtimeSpoolFatalKind::RecoveryCorruptFrame);
        assert_eq!(std::fs::read(&config.wal_path).unwrap(), before);
    }

    #[test]
    fn checkpoint_requires_exact_contiguous_pending_prefix() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_5", 6);
        let opened =
            RealtimeMessageSpool::open(config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_5")).unwrap();
        opened.spool.append(&admission("message-1", "one")).unwrap();
        opened.spool.append(&admission("message-2", "two")).unwrap();
        let recovered = opened.spool.recover_pending().unwrap();
        let second = RealtimeSpoolReplayProgress::pending(recovered[1].clone(), []).with_ingestion(
            personal_secretary::IngestMessageOutcome::Duplicate {
                source_event_id: personal_secretary::SourceEventId::new("event-2").unwrap(),
            },
        );
        let invalid = checkpointable_prefix(opened.spool.generation_id().clone(), &[second]);

        assert_eq!(
            opened.spool.advance_checkpoint(&invalid).unwrap_err().kind,
            RealtimeSpoolFatalKind::CheckpointFailed
        );
    }

    #[test]
    fn checkpoint_and_compact_keep_only_uncheckpointed_frames() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_6", 7);
        let config = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_6");
        let opened = RealtimeMessageSpool::open(config.clone()).unwrap();
        opened.spool.append(&admission("message-1", "one")).unwrap();
        opened.spool.append(&admission("message-2", "two")).unwrap();
        let recovered = opened.spool.recover_pending().unwrap();
        let first = RealtimeSpoolReplayProgress::pending(recovered[0].clone(), []).with_ingestion(
            personal_secretary::IngestMessageOutcome::Duplicate {
                source_event_id: personal_secretary::SourceEventId::new("event-1").unwrap(),
            },
        );
        let prefix = checkpointable_prefix(opened.spool.generation_id().clone(), &[first]);
        opened.spool.advance_checkpoint(&prefix).unwrap();
        assert_eq!(opened.spool.recover_pending().unwrap().len(), 1);
        opened.spool.compact().unwrap();
        let compacted_len = std::fs::metadata(&config.wal_path).unwrap().len();
        drop(opened);

        let reopened = RealtimeMessageSpool::open(config).unwrap();
        let pending = reopened.spool.recover_pending().unwrap();

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].record_id(), recovered[1].record_id());
        assert!(compacted_len < ACTIVE_WAL_BYTES);
    }

    #[test]
    fn process_lock_prevents_two_spool_owners() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_7", 8);
        let config = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_7");
        let first = RealtimeMessageSpool::open(config.clone()).unwrap();
        let error = RealtimeMessageSpool::open(config).err().unwrap();

        assert_eq!(error.kind, RealtimeSpoolFatalKind::LockUnavailable);
        drop(first);
    }

    #[test]
    fn frame_authentication_is_bound_to_wal_generation() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_8", 9);
        let config = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_8");
        let opened = RealtimeMessageSpool::open(config.clone()).unwrap();
        opened.spool.append(&admission("message-1", "one")).unwrap();
        drop(opened);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&config.wal_path)
            .unwrap();
        file.seek(SeekFrom::Start((4 + 1 + KEY_ID_LEN) as u64))
            .unwrap();
        file.write_all(Uuid::new_v4().as_bytes()).unwrap();
        file.sync_all().unwrap();

        let error = RealtimeMessageSpool::open(config).err().unwrap();

        assert_eq!(error.kind, RealtimeSpoolFatalKind::RecoveryCorruptFrame);
        assert_eq!(error.stage, "frame_auth");
    }

    #[test]
    fn configured_frame_limit_rejects_before_wal_append() {
        let temp = TempDir::new().unwrap();
        install_key("QQBOT_TEST_MESSAGE_SPOOL_KEY_9", 10);
        let mut config = config(&temp, "QQBOT_TEST_MESSAGE_SPOOL_KEY_9");
        config.max_frame_plaintext = 128;
        let opened = RealtimeMessageSpool::open(config.clone()).unwrap();
        let before = std::fs::metadata(&config.wal_path).unwrap().len();

        let error = opened
            .spool
            .append(&admission("message-1", &"x".repeat(512)))
            .unwrap_err();

        assert_eq!(error.kind, RealtimeSpoolFatalKind::CapacityExhausted);
        assert_eq!(std::fs::metadata(&config.wal_path).unwrap().len(), before);
    }
}
