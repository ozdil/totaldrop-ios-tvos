// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Hardened App Group (`group.app.totaldrop.ios`) manifest parser, path-confinement
//! re-validator, and `BGContinuedProcessingTask` expiration guard for `TotalDropIOS`.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::Value;
use totaldrop_proto::{chunk_count, limits, validate_path};

use crate::{
    AppleMobileL10n, BG_TRANSFER_TASK_ID, BackgroundDirection, BackgroundTransferCheckpoint,
    IOS_APP_GROUP_ID, IosShareManifest, ManifestEntry, MobileStopToken, ShareExtensionSkipReason,
};

/// Upper bound on serialized App Group JSON handoff files (`4 MiB`) to prevent
/// memory exhaustion (`Jetsam`) if a container file is corrupted.
pub const MAX_HANDOFF_JSON_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of unenumerated `skippedCount` placeholders synthesized when a
/// minimal Swift `ShareExtension` manifest omits the detailed `skipped` array.
const MAX_SYNTHETIC_SKIPPED: usize = 1_000;

/// Maximum UTF-8 byte length accepted for a localized `notice` banner in App Group JSON.
const MAX_NOTICE_BYTES: usize = 4_096;

/// Validation or parsing failure when consuming an App Group handoff file on iOS/iPadOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffError {
    /// Raw JSON exceeded [`MAX_HANDOFF_JSON_BYTES`].
    PayloadTooLarge {
        /// Observed byte length.
        byte_len: usize,
    },
    /// Input was not valid UTF-8 JSON object syntax.
    MalformedJson(String),
    /// Supplied `app_group_root` was not a clean absolute directory path.
    InvalidAppGroupRoot,
    /// Manifest `appGroup` did not match [`IOS_APP_GROUP_ID`].
    WrongAppGroup {
        /// Untrusted `appGroup` identifier found in JSON.
        found: String,
    },
    /// Required JSON property was missing.
    MissingField {
        /// Name of the missing property.
        field: &'static str,
    },
    /// JSON property had an unexpected type or out-of-range value.
    InvalidFieldType {
        /// Name of the invalid property.
        field: &'static str,
    },
    /// Entry count exceeded [`totaldrop_proto::limits::MAX_ENTRIES`].
    TooManyEntries {
        /// Number of entries present in the JSON array.
        count: usize,
    },
    /// Manifest marked `rejectedBusy: true` simultaneously carried non-empty `entries`.
    BusyManifestHasEntries,
    /// `stagedPath` attempted `.` or `..` directory traversal.
    PathTraversal {
        /// Rejected raw path string.
        path: String,
    },
    /// `stagedPath` pointed outside `app_group_root` (or to `app_group_root` itself).
    OutsideAppGroup {
        /// Rejected raw path string.
        path: String,
    },
    /// Relative segments inside `app_group_root` failed [`totaldrop_proto::validate_path`].
    UnsafeStagedPath {
        /// Rejected raw path string.
        path: String,
    },
    /// Entry `safeName` failed [`totaldrop_proto::validate_path`].
    UnsafeEntryName {
        /// Rejected file name.
        name: String,
    },
    /// Entry `collisionKey` did not match the canonical case-folded `safeName`.
    CollisionKeyMismatch {
        /// Entry `safeName` whose `collisionKey` was tampered with.
        name: String,
    },
    /// Two entries in the manifest collided case-insensitively.
    DuplicateCollisionKey {
        /// Duplicate case-folded collision key.
        collision_key: String,
    },
    /// Entry `byteSize` exceeded [`totaldrop_proto::limits::MAX_CHUNKS`].
    EntryTooLarge {
        /// Entry `safeName`.
        name: String,
        /// Rejected byte size.
        byte_size: u64,
    },
    /// Entry `chunks` field disagreed with [`totaldrop_proto::chunk_count`].
    ChunkCountMismatch {
        /// Entry `safeName`.
        name: String,
        /// Expected protocol chunk count.
        expected: u64,
        /// Tampered chunk count found in JSON.
        found: u64,
    },
    /// Sum of entry `byteSize` values overflowed `u64`.
    TotalBytesOverflow,
    /// Top-level `totalBytes` disagreed with the sum of entry `byteSize` values.
    TotalBytesMismatch {
        /// Computed sum across entries.
        expected: u64,
        /// Value recorded in JSON.
        found: u64,
    },
    /// `skippedCount` disagreed with the length of the `skipped` array.
    SkippedCountMismatch {
        /// Length of `skipped` array.
        expected: usize,
        /// Value recorded in `skippedCount`.
        found: usize,
    },
    /// Unknown `ShareExtensionSkipReason` tag in `skipped`.
    UnknownSkipReason {
        /// Unrecognized reason tag.
        reason: String,
    },
    /// Checkpoint `taskIdentifier` did not match [`BG_TRANSFER_TASK_ID`].
    InvalidCheckpointTaskId {
        /// Found identifier.
        found: String,
    },
    /// Checkpoint `direction` was neither `"sending"` nor `"receiving"`.
    InvalidCheckpointDirection {
        /// Found direction string.
        found: String,
    },
}

impl core::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PayloadTooLarge { byte_len } => {
                write!(f, "App Group JSON payload too large ({byte_len} bytes)")
            }
            Self::MalformedJson(detail) => write!(f, "malformed App Group JSON: {detail}"),
            Self::InvalidAppGroupRoot => {
                f.write_str("app_group_root must be an absolute path without `.` or `..`")
            }
            Self::WrongAppGroup { found } => {
                write!(
                    f,
                    "unexpected App Group `{found}` (expected `{IOS_APP_GROUP_ID}`)"
                )
            }
            Self::MissingField { field } => write!(f, "missing required JSON field `{field}`"),
            Self::InvalidFieldType { field } => {
                write!(f, "invalid type or out-of-bounds value for `{field}`")
            }
            Self::TooManyEntries { count } => {
                write!(f, "manifest has {count} entries, exceeding protocol limit")
            }
            Self::BusyManifestHasEntries => {
                f.write_str("busy manifest must not carry staged entries")
            }
            Self::PathTraversal { path } => {
                write!(f, "staged path contains traversal component: {path}")
            }
            Self::OutsideAppGroup { path } => {
                write!(f, "staged path escapes App Group container: {path}")
            }
            Self::UnsafeStagedPath { path } => {
                write!(f, "staged path failed protocol path safety rules: {path}")
            }
            Self::UnsafeEntryName { name } => {
                write!(f, "entry name failed protocol path safety rules: {name}")
            }
            Self::CollisionKeyMismatch { name } => {
                write!(
                    f,
                    "collisionKey does not match canonical safeName for `{name}`"
                )
            }
            Self::DuplicateCollisionKey { collision_key } => {
                write!(
                    f,
                    "duplicate case-colliding entry `{collision_key}` in manifest"
                )
            }
            Self::EntryTooLarge { name, byte_size } => {
                write!(f, "entry `{name}` ({byte_size} bytes) exceeds chunk limit")
            }
            Self::ChunkCountMismatch {
                name,
                expected,
                found,
            } => write!(
                f,
                "entry `{name}` chunk count {found} does not match expected {expected}"
            ),
            Self::TotalBytesOverflow => f.write_str("manifest totalBytes overflowed u64"),
            Self::TotalBytesMismatch { expected, found } => {
                write!(
                    f,
                    "manifest totalBytes {found} does not match sum {expected}"
                )
            }
            Self::SkippedCountMismatch { expected, found } => {
                write!(
                    f,
                    "manifest skippedCount {found} does not match skipped list length {expected}"
                )
            }
            Self::UnknownSkipReason { reason } => {
                write!(f, "unrecognized ShareExtensionSkipReason `{reason}`")
            }
            Self::InvalidCheckpointTaskId { found } => {
                write!(f, "unexpected background task identifier `{found}`")
            }
            Self::InvalidCheckpointDirection { found } => {
                write!(f, "unexpected background transfer direction `{found}`")
            }
        }
    }
}

impl std::error::Error for HandoffError {}

pub(crate) const fn skip_reason_slug(reason: &ShareExtensionSkipReason) -> &'static str {
    match reason {
        ShareExtensionSkipReason::SecurityScopeDenied => "securityScopeDenied",
        ShareExtensionSkipReason::OutsideAppGroupContainer => "outsideAppGroupContainer",
        ShareExtensionSkipReason::UnsafeName => "unsafeName",
        ShareExtensionSkipReason::DuplicateNameCollision => "duplicateNameCollision",
        ShareExtensionSkipReason::EntryLimitReached => "entryLimitReached",
    }
}

fn parse_skip_reason_slug(slug: &str) -> Result<ShareExtensionSkipReason, HandoffError> {
    match slug {
        "securityScopeDenied" => Ok(ShareExtensionSkipReason::SecurityScopeDenied),
        "outsideAppGroupContainer" => Ok(ShareExtensionSkipReason::OutsideAppGroupContainer),
        "unsafeName" => Ok(ShareExtensionSkipReason::UnsafeName),
        "duplicateNameCollision" => Ok(ShareExtensionSkipReason::DuplicateNameCollision),
        "entryLimitReached" => Ok(ShareExtensionSkipReason::EntryLimitReached),
        other => Err(HandoffError::UnknownSkipReason {
            reason: other.to_owned(),
        }),
    }
}

fn validate_app_group_root(app_group_root: &Path) -> Result<(), HandoffError> {
    if !app_group_root.is_absolute()
        || app_group_root.as_os_str().is_empty()
        || app_group_root
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(HandoffError::InvalidAppGroupRoot);
    }
    Ok(())
}

fn validate_confined_staged_path(
    raw_path: &str,
    app_group_root: &Path,
) -> Result<PathBuf, HandoffError> {
    if raw_path.is_empty() || raw_path.contains('\0') {
        return Err(HandoffError::UnsafeStagedPath {
            path: raw_path.to_owned(),
        });
    }
    let path = PathBuf::from(raw_path);
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(HandoffError::PathTraversal {
            path: raw_path.to_owned(),
        });
    }
    if !path.is_absolute() {
        return Err(HandoffError::OutsideAppGroup {
            path: raw_path.to_owned(),
        });
    }
    let Ok(relative) = path.strip_prefix(app_group_root) else {
        return Err(HandoffError::OutsideAppGroup {
            path: raw_path.to_owned(),
        });
    };

    let mut rel_segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(seg) => {
                let Some(text) = seg.to_str() else {
                    return Err(HandoffError::UnsafeStagedPath {
                        path: raw_path.to_owned(),
                    });
                };
                rel_segments.push(text);
            }
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(HandoffError::PathTraversal {
                    path: raw_path.to_owned(),
                });
            }
        }
    }

    if rel_segments.is_empty() {
        return Err(HandoffError::OutsideAppGroup {
            path: raw_path.to_owned(),
        });
    }
    if validate_path(&rel_segments).is_err() {
        return Err(HandoffError::UnsafeStagedPath {
            path: raw_path.to_owned(),
        });
    }
    Ok(path)
}

/// Parses and re-validates an App Group JSON manifest written by
/// [`IosShareManifest::to_app_group_json`] or `ShareExtensionManifestStaging`.
///
/// Every entry is re-checked against `app_group_root` so a stale or tampered
/// `manifest.json` inside `group.app.totaldrop.ios` cannot smuggle `..` parent
/// traversal, paths outside `app_group_root`, unsafe/bidi-spoofed file names,
/// duplicate case-colliding names, or inconsistent byte/chunk totals.
///
/// # Errors
///
/// Returns a [`HandoffError`] if JSON parsing fails or any security invariant is violated.
pub fn parse_ios_share_manifest_json(
    raw_json: &str,
    app_group_root: &Path,
) -> Result<IosShareManifest, HandoffError> {
    validate_app_group_root(app_group_root)?;
    if raw_json.len() > MAX_HANDOFF_JSON_BYTES {
        return Err(HandoffError::PayloadTooLarge {
            byte_len: raw_json.len(),
        });
    }

    let root_val: Value =
        serde_json::from_str(raw_json).map_err(|e| HandoffError::MalformedJson(e.to_string()))?;
    let obj = root_val
        .as_object()
        .ok_or_else(|| HandoffError::MalformedJson("top-level value must be an object".into()))?;

    let app_group = obj
        .get("appGroup")
        .ok_or(HandoffError::MissingField { field: "appGroup" })?
        .as_str()
        .ok_or(HandoffError::InvalidFieldType { field: "appGroup" })?;
    if app_group != IOS_APP_GROUP_ID {
        return Err(HandoffError::WrongAppGroup {
            found: app_group.to_owned(),
        });
    }

    let rejected_busy = match obj.get("rejectedBusy") {
        None => false,
        Some(v) => v.as_bool().ok_or(HandoffError::InvalidFieldType {
            field: "rejectedBusy",
        })?,
    };

    let raw_entries = obj
        .get("entries")
        .ok_or(HandoffError::MissingField { field: "entries" })?
        .as_array()
        .ok_or(HandoffError::InvalidFieldType { field: "entries" })?;
    if raw_entries.len() > limits::MAX_ENTRIES {
        return Err(HandoffError::TooManyEntries {
            count: raw_entries.len(),
        });
    }
    if rejected_busy && !raw_entries.is_empty() {
        return Err(HandoffError::BusyManifestHasEntries);
    }

    let mut entries = Vec::with_capacity(raw_entries.len());
    let mut seen_keys = BTreeSet::new();
    let mut computed_total: u64 = 0;

    for item in raw_entries {
        let entry_obj = item
            .as_object()
            .ok_or(HandoffError::InvalidFieldType { field: "entries" })?;

        let raw_staged = entry_obj
            .get("stagedPath")
            .ok_or(HandoffError::MissingField {
                field: "stagedPath",
            })?
            .as_str()
            .ok_or(HandoffError::InvalidFieldType {
                field: "stagedPath",
            })?;
        let staged_path = validate_confined_staged_path(raw_staged, app_group_root)?;

        let raw_safe_name = entry_obj
            .get("safeName")
            .ok_or(HandoffError::MissingField { field: "safeName" })?
            .as_str()
            .ok_or(HandoffError::InvalidFieldType { field: "safeName" })?;
        let Ok(safe_path) = validate_path(&[raw_safe_name]) else {
            return Err(HandoffError::UnsafeEntryName {
                name: raw_safe_name.to_owned(),
            });
        };
        let safe_name = safe_path.segments().first().cloned().unwrap_or_default();
        let expected_collision_key = safe_path.collision_key();

        if let Some(key_val) = entry_obj.get("collisionKey") {
            let provided_key = key_val.as_str().ok_or(HandoffError::InvalidFieldType {
                field: "collisionKey",
            })?;
            if provided_key != expected_collision_key {
                return Err(HandoffError::CollisionKeyMismatch { name: safe_name });
            }
        }
        if !seen_keys.insert(expected_collision_key.clone()) {
            return Err(HandoffError::DuplicateCollisionKey {
                collision_key: expected_collision_key,
            });
        }

        let byte_size = entry_obj
            .get("byteSize")
            .ok_or(HandoffError::MissingField { field: "byteSize" })?
            .as_u64()
            .ok_or(HandoffError::InvalidFieldType { field: "byteSize" })?;
        let Some(expected_chunks) = chunk_count(byte_size) else {
            return Err(HandoffError::EntryTooLarge {
                name: safe_name,
                byte_size,
            });
        };

        if let Some(chunks_val) = entry_obj.get("chunks") {
            let found_chunks = chunks_val
                .as_u64()
                .ok_or(HandoffError::InvalidFieldType { field: "chunks" })?;
            if found_chunks != expected_chunks {
                return Err(HandoffError::ChunkCountMismatch {
                    name: safe_name,
                    expected: expected_chunks,
                    found: found_chunks,
                });
            }
        }

        computed_total = computed_total
            .checked_add(byte_size)
            .ok_or(HandoffError::TotalBytesOverflow)?;

        entries.push(ManifestEntry {
            staged_path,
            safe_name,
            collision_key: expected_collision_key,
            byte_size,
            chunks: expected_chunks,
        });
    }

    if let Some(total_val) = obj.get("totalBytes") {
        let declared_total = total_val.as_u64().ok_or(HandoffError::InvalidFieldType {
            field: "totalBytes",
        })?;
        if declared_total != computed_total {
            return Err(HandoffError::TotalBytesMismatch {
                expected: computed_total,
                found: declared_total,
            });
        }
    }

    let skipped_count = match obj.get("skippedCount") {
        None => None,
        Some(v) => {
            let n = v.as_u64().ok_or(HandoffError::InvalidFieldType {
                field: "skippedCount",
            })?;
            let count = usize::try_from(n).map_err(|_| HandoffError::InvalidFieldType {
                field: "skippedCount",
            })?;
            if count > limits::MAX_ENTRIES {
                return Err(HandoffError::InvalidFieldType {
                    field: "skippedCount",
                });
            }
            Some(count)
        }
    };

    let skipped = match obj.get("skipped") {
        Some(raw_skipped) => {
            let arr = raw_skipped
                .as_array()
                .ok_or(HandoffError::InvalidFieldType { field: "skipped" })?;
            if arr.len() > limits::MAX_ENTRIES {
                return Err(HandoffError::InvalidFieldType { field: "skipped" });
            }
            let mut parsed_skipped = Vec::with_capacity(arr.len());
            for sk in arr {
                let sk_obj = sk
                    .as_object()
                    .ok_or(HandoffError::InvalidFieldType { field: "skipped" })?;
                let orig = sk_obj
                    .get("originalName")
                    .ok_or(HandoffError::MissingField {
                        field: "originalName",
                    })?
                    .as_str()
                    .ok_or(HandoffError::InvalidFieldType {
                        field: "originalName",
                    })?;
                let reason_str = sk_obj
                    .get("reason")
                    .ok_or(HandoffError::MissingField { field: "reason" })?
                    .as_str()
                    .ok_or(HandoffError::InvalidFieldType { field: "reason" })?;
                let reason = parse_skip_reason_slug(reason_str)?;
                parsed_skipped.push((totaldrop_l10n::reveal_controls(orig), reason));
            }
            if let Some(declared_count) = skipped_count
                && declared_count != parsed_skipped.len()
            {
                return Err(HandoffError::SkippedCountMismatch {
                    expected: parsed_skipped.len(),
                    found: declared_count,
                });
            }
            parsed_skipped
        }
        None => match skipped_count {
            None | Some(0) => Vec::new(),
            Some(count) if count <= MAX_SYNTHETIC_SKIPPED => (0..count)
                .map(|idx| {
                    (
                        format!("skipped-{idx}"),
                        ShareExtensionSkipReason::SecurityScopeDenied,
                    )
                })
                .collect(),
            Some(_) => {
                return Err(HandoffError::InvalidFieldType {
                    field: "skippedCount",
                });
            }
        },
    };

    let localized_notice = match obj.get("notice") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let s = v
                .as_str()
                .ok_or(HandoffError::InvalidFieldType { field: "notice" })?;
            if s.len() > MAX_NOTICE_BYTES {
                return Err(HandoffError::InvalidFieldType { field: "notice" });
            }
            Some(totaldrop_l10n::reveal_controls(s))
        }
    };

    Ok(IosShareManifest {
        app_group: app_group.to_owned(),
        entries,
        total_bytes: computed_total,
        skipped,
        localized_notice,
        rejected_busy,
    })
}

/// Parses a persisted [`BackgroundTransferCheckpoint`] JSON written during
/// `BGContinuedProcessingTask` execution or system expiration.
///
/// # Errors
///
/// Returns [`HandoffError`] if the JSON is malformed or carries invalid task metadata.
pub fn parse_background_checkpoint_json(
    raw_json: &str,
) -> Result<BackgroundTransferCheckpoint, HandoffError> {
    if raw_json.len() > MAX_HANDOFF_JSON_BYTES {
        return Err(HandoffError::PayloadTooLarge {
            byte_len: raw_json.len(),
        });
    }
    let val: Value =
        serde_json::from_str(raw_json).map_err(|e| HandoffError::MalformedJson(e.to_string()))?;
    let obj = val
        .as_object()
        .ok_or_else(|| HandoffError::MalformedJson("checkpoint must be a JSON object".into()))?;

    let task_identifier = obj
        .get("taskIdentifier")
        .ok_or(HandoffError::MissingField {
            field: "taskIdentifier",
        })?
        .as_str()
        .ok_or(HandoffError::InvalidFieldType {
            field: "taskIdentifier",
        })?;
    if task_identifier != BG_TRANSFER_TASK_ID {
        return Err(HandoffError::InvalidCheckpointTaskId {
            found: task_identifier.to_owned(),
        });
    }

    let dir_str = obj
        .get("direction")
        .ok_or(HandoffError::MissingField { field: "direction" })?
        .as_str()
        .ok_or(HandoffError::InvalidFieldType { field: "direction" })?;
    let direction = match dir_str {
        "sending" => BackgroundDirection::Sending,
        "receiving" => BackgroundDirection::Receiving,
        other => {
            return Err(HandoffError::InvalidCheckpointDirection {
                found: other.to_owned(),
            });
        }
    };

    let done_bytes = obj
        .get("doneBytes")
        .ok_or(HandoffError::MissingField { field: "doneBytes" })?
        .as_u64()
        .ok_or(HandoffError::InvalidFieldType { field: "doneBytes" })?;
    let total_bytes = obj
        .get("totalBytes")
        .ok_or(HandoffError::MissingField {
            field: "totalBytes",
        })?
        .as_u64()
        .ok_or(HandoffError::InvalidFieldType {
            field: "totalBytes",
        })?;
    let completed_chunks = obj
        .get("completedChunks")
        .ok_or(HandoffError::MissingField {
            field: "completedChunks",
        })?
        .as_u64()
        .ok_or(HandoffError::InvalidFieldType {
            field: "completedChunks",
        })?;
    let total_chunks = obj
        .get("totalChunks")
        .ok_or(HandoffError::MissingField {
            field: "totalChunks",
        })?
        .as_u64()
        .ok_or(HandoffError::InvalidFieldType {
            field: "totalChunks",
        })?;

    let percent_u64 = obj
        .get("percent")
        .ok_or(HandoffError::MissingField { field: "percent" })?
        .as_u64()
        .ok_or(HandoffError::InvalidFieldType { field: "percent" })?;
    let percent = u8::try_from(percent_u64)
        .ok()
        .filter(|p| *p <= 100)
        .ok_or(HandoffError::InvalidFieldType { field: "percent" })?;

    let notification_title = obj
        .get("notificationTitle")
        .ok_or(HandoffError::MissingField {
            field: "notificationTitle",
        })?
        .as_str()
        .ok_or(HandoffError::InvalidFieldType {
            field: "notificationTitle",
        })?;
    let notification_subtitle = obj
        .get("notificationSubtitle")
        .ok_or(HandoffError::MissingField {
            field: "notificationSubtitle",
        })?
        .as_str()
        .ok_or(HandoffError::InvalidFieldType {
            field: "notificationSubtitle",
        })?;
    let progress_summary = obj
        .get("progressSummary")
        .ok_or(HandoffError::MissingField {
            field: "progressSummary",
        })?
        .as_str()
        .ok_or(HandoffError::InvalidFieldType {
            field: "progressSummary",
        })?;
    let expired_by_system = obj
        .get("expiredBySystem")
        .ok_or(HandoffError::MissingField {
            field: "expiredBySystem",
        })?
        .as_bool()
        .ok_or(HandoffError::InvalidFieldType {
            field: "expiredBySystem",
        })?;

    Ok(BackgroundTransferCheckpoint {
        task_identifier: task_identifier.to_owned(),
        direction,
        done_bytes: if total_bytes > 0 {
            done_bytes.min(total_bytes)
        } else {
            done_bytes
        },
        total_bytes,
        completed_chunks,
        total_chunks,
        percent,
        notification_title: totaldrop_l10n::reveal_controls(notification_title),
        notification_subtitle: totaldrop_l10n::reveal_controls(notification_subtitle),
        progress_summary: totaldrop_l10n::reveal_controls(progress_summary),
        expired_by_system,
    })
}

/// Snapshot emitted when iOS invokes `BGContinuedProcessingTask`'s `expirationHandler`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemExpirationReport {
    /// Captured checkpoint marked with `expired_by_system: true`.
    pub checkpoint: BackgroundTransferCheckpoint,
    /// UTF-8 JSON payload ready for atomic write into `group.app.totaldrop.ios`
    /// so `TotalDropIOS` can notify the user on resume.
    pub checkpoint_json: String,
}

#[derive(Debug)]
struct GuardState {
    done_bytes: AtomicU64,
    total_bytes: AtomicU64,
    expired_by_system: AtomicBool,
}

/// Thread-safe guard linking a [`MobileStopToken`] with live progress counters
/// so an iOS `BGContinuedProcessingTask` expiration callback atomically stops
/// the transfer and emits a resumable App Group checkpoint JSON.
#[derive(Debug, Clone)]
pub struct BackgroundTaskGuard {
    direction: BackgroundDirection,
    token: MobileStopToken,
    state: Arc<GuardState>,
}

impl BackgroundTaskGuard {
    /// Creates a new background task guard with a fresh [`MobileStopToken`].
    #[must_use]
    pub fn new(direction: BackgroundDirection, total_bytes: u64) -> Self {
        Self::with_stop_token(direction, total_bytes, MobileStopToken::new())
    }

    /// Creates a background task guard bound to an existing [`MobileStopToken`].
    #[must_use]
    pub fn with_stop_token(
        direction: BackgroundDirection,
        total_bytes: u64,
        token: MobileStopToken,
    ) -> Self {
        Self {
            direction,
            token,
            state: Arc::new(GuardState {
                done_bytes: AtomicU64::new(0),
                total_bytes: AtomicU64::new(total_bytes),
                expired_by_system: AtomicBool::new(false),
            }),
        }
    }

    /// Direction of the guarded background transfer.
    #[must_use]
    pub const fn direction(&self) -> BackgroundDirection {
        self.direction
    }

    /// Underlying [`MobileStopToken`] shared with `prepare_mobile_send` or `connect_mobile_receive`.
    #[must_use]
    pub const fn stop_token(&self) -> &MobileStopToken {
        &self.token
    }

    /// Updates transferred and total byte counters from `totaldrop_core::Event::Progress`.
    pub fn record_progress(&self, done_bytes: u64, total_bytes: u64) {
        let clamped = if total_bytes > 0 {
            done_bytes.min(total_bytes)
        } else {
            done_bytes
        };
        self.state.total_bytes.store(total_bytes, Ordering::SeqCst);
        self.state.done_bytes.store(clamped, Ordering::SeqCst);
    }

    /// Whether `BGTask.expirationHandler` has fired for this session.
    #[must_use]
    pub fn is_expired_by_system(&self) -> bool {
        self.state.expired_by_system.load(Ordering::SeqCst)
    }

    /// Whether the underlying [`MobileStopToken`] has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Captures a point-in-time [`BackgroundTransferCheckpoint`] without cancelling the transfer.
    #[must_use]
    pub fn checkpoint(&self, l10n: &AppleMobileL10n) -> BackgroundTransferCheckpoint {
        let done = self.state.done_bytes.load(Ordering::SeqCst);
        let total = self.state.total_bytes.load(Ordering::SeqCst);
        let expired = self.state.expired_by_system.load(Ordering::SeqCst);
        BackgroundTransferCheckpoint::capture(self.direction, done, total, expired, l10n)
    }

    /// Invoked from iOS `BGContinuedProcessingTask` / `BGProcessingTask` `expirationHandler`:
    /// atomically marks system expiration, cancels [`MobileStopToken`], and returns the
    /// `expired_by_system: true` checkpoint + JSON for App Group persistence.
    #[must_use]
    pub fn on_system_expiration(&self, l10n: &AppleMobileL10n) -> SystemExpirationReport {
        self.state.expired_by_system.store(true, Ordering::SeqCst);
        self.token.cancel();
        let done = self.state.done_bytes.load(Ordering::SeqCst);
        let total = self.state.total_bytes.load(Ordering::SeqCst);
        let checkpoint =
            BackgroundTransferCheckpoint::capture(self.direction, done, total, true, l10n);
        let checkpoint_json = checkpoint.to_json();
        SystemExpirationReport {
            checkpoint,
            checkpoint_json,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use serde_json::json;
    use totaldrop_core::Error as CoreError;

    use super::*;
    use crate::{
        MobileActivityState, MobileTransferConfig, SecurityScopedCandidate,
        build_ios_share_manifest, connect_mobile_receive, prepare_mobile_send,
    };

    fn en_l10n() -> AppleMobileL10n {
        AppleMobileL10n::from_apple_preferences(&["en-US".to_owned()], Some("en_US"))
    }

    #[test]
    fn app_group_manifest_json_round_trips_entries_nfd_and_skipped_metadata() {
        let l10n = en_l10n();
        let group_root =
            Path::new("/private/var/mobile/Containers/Shared/AppGroup/UUID/SharedInbox");
        let candidates = vec![
            SecurityScopedCandidate {
                staged_path: group_root.join("0/Vacances_e\u{0301}te\u{0301}.heic"),
                original_name: "Vacances_e\u{0301}te\u{0301}.heic".to_owned(),
                byte_size: 2_500_000,
                security_scope_granted: true,
            },
            SecurityScopedCandidate {
                staged_path: group_root.join("1/Notes.pdf"),
                original_name: "Notes.pdf".to_owned(),
                byte_size: 0,
                security_scope_granted: true,
            },
            SecurityScopedCandidate {
                staged_path: group_root.join("2/locked.mov"),
                original_name: "locked.mov".to_owned(),
                byte_size: 8_192,
                security_scope_granted: false,
            },
        ];

        let original =
            build_ios_share_manifest(group_root, &candidates, MobileActivityState::Idle, &l10n);
        let json_payload = original.to_app_group_json();
        let restored = parse_ios_share_manifest_json(&json_payload, group_root).unwrap();
        assert_eq!(restored, original);
        assert_eq!(restored.entries.len(), 2);
        assert_eq!(restored.entries[0].safe_name, "Vacances_été.heic");
        assert_eq!(restored.entries[0].chunks, 3);
        assert_eq!(restored.entries[1].chunks, 1);
        assert_eq!(restored.skipped.len(), 1);

        // Minimal Swift ShareExtension JSON (without `chunks`, `totalBytes`, or `skipped` array)
        // also parses cleanly and recomputes chunk counts and total bytes.
        let minimal_swift_json = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "skippedCount": 1,
            "entries": [
                {
                    "stagedPath": "/private/var/mobile/Containers/Shared/AppGroup/UUID/SharedInbox/0/Deck.key",
                    "safeName": "Deck.key",
                    "collisionKey": "deck.key",
                    "byteSize": 1_048_577_u64
                }
            ]
        })
        .to_string();
        let from_swift = parse_ios_share_manifest_json(&minimal_swift_json, group_root).unwrap();
        assert_eq!(from_swift.entries.len(), 1);
        assert_eq!(from_swift.entries[0].chunks, 2);
        assert_eq!(from_swift.total_bytes, 1_048_577);
        assert_eq!(from_swift.skipped.len(), 1);
    }

    #[test]
    fn rejects_tampered_staged_path_outside_app_group_and_parent_traversal() {
        let group_root = Path::new("/AppGroup/SharedInbox");

        // Escape to /private/var/mobile/Library/SMS/sms.db
        let outside = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "entries": [{
                "stagedPath": "/private/var/mobile/Library/SMS/sms.db",
                "safeName": "sms.db",
                "collisionKey": "sms.db",
                "byteSize": 4096
            }]
        })
        .to_string();
        assert!(matches!(
            parse_ios_share_manifest_json(&outside, group_root),
            Err(HandoffError::OutsideAppGroup { .. })
        ));

        // Sibling prefix attack (/AppGroup/SharedInboxEvil vs /AppGroup/SharedInbox)
        let sibling_prefix = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "entries": [{
                "stagedPath": "/AppGroup/SharedInboxEvil/0/file.pdf",
                "safeName": "file.pdf",
                "collisionKey": "file.pdf",
                "byteSize": 128
            }]
        })
        .to_string();
        assert!(matches!(
            parse_ios_share_manifest_json(&sibling_prefix, group_root),
            Err(HandoffError::OutsideAppGroup { .. })
        ));

        // Parent-directory traversal (`..`) inside `app_group_root`
        let traversal = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "entries": [{
                "stagedPath": "/AppGroup/SharedInbox/0/../../Library/Preferences/secret.plist",
                "safeName": "secret.plist",
                "collisionKey": "secret.plist",
                "byteSize": 512
            }]
        })
        .to_string();
        assert!(matches!(
            parse_ios_share_manifest_json(&traversal, group_root),
            Err(HandoffError::PathTraversal { .. })
        ));

        // Staged path pointing to `app_group_root` itself with no child file
        let root_itself = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "entries": [{
                "stagedPath": "/AppGroup/SharedInbox",
                "safeName": "SharedInbox",
                "collisionKey": "sharedinbox",
                "byteSize": 64
            }]
        })
        .to_string();
        assert!(matches!(
            parse_ios_share_manifest_json(&root_itself, group_root),
            Err(HandoffError::OutsideAppGroup { .. })
        ));
    }

    #[test]
    fn rejects_wrong_app_group_id_unsafe_names_collisions_and_tampered_totals() {
        let group_root = Path::new("/AppGroup/SharedInbox");

        let wrong_group = json!({
            "appGroup": "group.com.untrusted.app",
            "entries": []
        })
        .to_string();
        assert_eq!(
            parse_ios_share_manifest_json(&wrong_group, group_root),
            Err(HandoffError::WrongAppGroup {
                found: "group.com.untrusted.app".to_owned(),
            })
        );

        assert_eq!(
            parse_ios_share_manifest_json(&wrong_group, Path::new("relative/inbox")),
            Err(HandoffError::InvalidAppGroupRoot)
        );

        // Hidden bidi override smuggled in `safeName`
        let bidi_name = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "entries": [{
                "stagedPath": "/AppGroup/SharedInbox/0/clean.pdf",
                "safeName": "invoice\u{202E}fdp.app",
                "byteSize": 100
            }]
        })
        .to_string();
        assert!(matches!(
            parse_ios_share_manifest_json(&bidi_name, group_root),
            Err(HandoffError::UnsafeEntryName { .. })
        ));

        // Duplicate case-colliding entries in tampered JSON
        let duplicate = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "entries": [
                {
                    "stagedPath": "/AppGroup/SharedInbox/0/Photo.HEIC",
                    "safeName": "Photo.HEIC",
                    "collisionKey": "photo.heic",
                    "byteSize": 100
                },
                {
                    "stagedPath": "/AppGroup/SharedInbox/1/photo.heic",
                    "safeName": "photo.heic",
                    "collisionKey": "photo.heic",
                    "byteSize": 200
                }
            ]
        })
        .to_string();
        assert_eq!(
            parse_ios_share_manifest_json(&duplicate, group_root),
            Err(HandoffError::DuplicateCollisionKey {
                collision_key: "photo.heic".to_owned(),
            })
        );

        // Tampered `totalBytes`
        let bad_total = json!({
            "appGroup": IOS_APP_GROUP_ID,
            "totalBytes": 999_999,
            "entries": [{
                "stagedPath": "/AppGroup/SharedInbox/0/Photo.HEIC",
                "safeName": "Photo.HEIC",
                "collisionKey": "photo.heic",
                "byteSize": 100
            }]
        })
        .to_string();
        assert_eq!(
            parse_ios_share_manifest_json(&bad_total, group_root),
            Err(HandoffError::TotalBytesMismatch {
                expected: 100,
                found: 999_999,
            })
        );
    }

    #[tokio::test]
    async fn background_task_guard_expiration_cancels_token_and_aborts_mobile_receive_and_send() {
        let l10n = AppleMobileL10n::from_apple_preferences(&["tr-TR".to_owned()], Some("tr_TR"));
        let total = limits::CHUNK_SIZE.saturating_mul(10);
        let done = limits::CHUNK_SIZE.saturating_mul(4);

        let guard = BackgroundTaskGuard::new(BackgroundDirection::Receiving, total);
        guard.record_progress(done, total);
        assert!(!guard.is_cancelled());
        assert!(!guard.is_expired_by_system());

        let live_cp = guard.checkpoint(&l10n);
        assert!(!live_cp.expired_by_system);
        assert_eq!(live_cp.percent, 40);
        assert_eq!(live_cp.completed_chunks, 4);
        assert_eq!(live_cp.total_chunks, 10);

        // Simulate iOS `BGContinuedProcessingTask.expirationHandler` firing.
        let report = guard.on_system_expiration(&l10n);
        assert!(guard.is_cancelled());
        assert!(guard.stop_token().is_cancelled());
        assert!(guard.is_expired_by_system());
        assert!(report.checkpoint.expired_by_system);
        assert_eq!(report.checkpoint.percent, 40);

        // Persisted JSON round-trips cleanly on App Group relaunch.
        let parsed_cp = parse_background_checkpoint_json(&report.checkpoint_json).unwrap();
        assert_eq!(parsed_cp, report.checkpoint);
        assert!(parsed_cp.expired_by_system);

        // Pre-cancelled `MobileStopToken` immediately aborts `connect_mobile_receive` and `prepare_mobile_send`.
        let cfg = MobileTransferConfig::default();
        let recv_err = connect_mobile_receive(
            &cfg,
            "https://totaldrop.app/r#7-harbor-rocket",
            guard.stop_token(),
        )
        .await
        .expect_err("pre-cancelled token must abort connect_mobile_receive");
        assert!(
            matches!(recv_err, CoreError::Stopped),
            "expected CoreError::Stopped, got {recv_err:?}"
        );

        let send_err = prepare_mobile_send(
            &cfg,
            vec![PathBuf::from("/nonexistent/staged.bin")],
            guard.stop_token(),
        )
        .await
        .expect_err("pre-cancelled token must abort prepare_mobile_send");
        assert!(
            matches!(send_err, CoreError::Stopped),
            "expected CoreError::Stopped, got {send_err:?}"
        );
    }
}
