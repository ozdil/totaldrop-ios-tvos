// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later
//
//! Safe Rust bridge for TotalDrop on iOS, iPadOS, and Apple tvOS (ADR 0009).
//!
//! Wraps [`totaldrop_core`], [`totaldrop_proto`], and [`totaldrop_l10n`] with
//! `#![forbid(unsafe_code)]` for Swift 6 strict-concurrency targets:
//! - Universal Links (`https://totaldrop.app/r#<code>`) and `totaldrop://r/<code>`
//!   parsing with active-transfer collision protection;
//! - iOS/iPadOS Share Extension security-scoped URL manifest construction and
//!   App Group (`group.app.totaldrop.ios`) JSON handoff;
//! - `BGContinuedProcessingTask` background transfer checkpoint metadata and
//!   expiration cancellation via [`totaldrop_core::Stop`];
//! - Apple tvOS 10-foot living-room pairing session state and Siri Remote
//!   directional Focus Engine transitions;
//! - 35-language localization with Dynamic Type layout metadata up to
//!   `accessibility5` and VoiceOver phonetic code cues.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use totaldrop_core::{
    Error as CoreError, IncomingOffer, PathOptions, PreparedSend, ReceiveOptions, SendOptions, Stop,
};
use totaldrop_l10n::{Arg, Domain, Strings};
use totaldrop_proto::{SafePath, TransferCode, chunk_count, limits, validate_path};

pub mod handoff;

pub use handoff::{
    BackgroundTaskGuard, HandoffError, MAX_HANDOFF_JSON_BYTES, SystemExpirationReport,
    parse_background_checkpoint_json, parse_ios_share_manifest_json,
};

/// App Group identifier shared between `TotalDropIOS` and `ShareExtension`.
pub const IOS_APP_GROUP_ID: &str = "group.app.totaldrop.ios";

/// Background task identifier registered in `BGTaskSchedulerPermittedIdentifiers`.
pub const BG_TRANSFER_TASK_ID: &str = "app.totaldrop.ios.transfer";

/// Universal Link base prefix served with `apple-app-site-association`.
pub const UNIVERSAL_LINK_PREFIX: &str = "https://totaldrop.app/r#";

/// Maximum characters rendered inline for received text on iOS and tvOS (#326).
pub const MAX_INLINE_TEXT_CHARS: usize = 16 * 1024;

/// Foreground or background transfer state of the iOS, iPadOS, or tvOS app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileActivityState {
    /// Ready to start a send or open an incoming link.
    Idle,
    /// Actively sending or waiting in the foreground or via `BGContinuedProcessingTask`.
    ActiveSend,
    /// Actively receiving or connecting.
    ActiveReceive,
    /// Waiting for user confirmation on an incoming offer or displaying received text.
    AwaitingUserDecision,
}

impl MobileActivityState {
    /// Whether the app can accept a new Universal Link or Share Extension handoff.
    #[must_use]
    pub const fn is_idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// Action returned to SwiftUI when an iOS/tvOS Universal Link or QR code is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UniversalLinkRoute {
    /// Navigate to Receive and connect using the extracted code.
    ConnectReceive {
        /// Normalized English transfer code (`7-harbor-rocket`).
        display_code: String,
        /// Canonical numeric code for CPace (`7-840-1499`).
        canonical_code: String,
        /// Reshareable Universal Link (`https://totaldrop.app/r#7-harbor-rocket`).
        universal_link: String,
        /// Hyphen-separated spoken words for VoiceOver (`"7, harbor, rocket"`).
        voiceover_spoken_code: String,
    },
    /// Another transfer or offer currently occupies the screen.
    Busy {
        /// Localized `linkBusy` warning.
        message: String,
    },
    /// Not a valid TotalDrop Universal Link or code.
    Invalid,
}

/// Parses an iOS/iPadOS/tvOS Universal Link (`https://totaldrop.app/r#<code>`),
/// custom scheme URL (`totaldrop://r/<code>`), or scanned QR code via
/// [`TransferCode::from_link`].
#[must_use]
pub fn parse_universal_link(
    raw_link: &str,
    state: MobileActivityState,
    l10n: &AppleMobileL10n,
) -> UniversalLinkRoute {
    let Some(code) = TransferCode::from_link(raw_link) else {
        return UniversalLinkRoute::Invalid;
    };
    if !state.is_idle() {
        return UniversalLinkRoute::Busy {
            message: l10n.text("linkBusy"),
        };
    }
    let display_code = code.to_english();
    let canonical_code = code.canonical();
    let universal_link = format!("{UNIVERSAL_LINK_PREFIX}{display_code}");
    let voiceover_spoken_code = format_voiceover_code(&display_code);
    UniversalLinkRoute::ConnectReceive {
        display_code,
        canonical_code,
        universal_link,
        voiceover_spoken_code,
    }
}

/// Formats a hyphenated code or word sequence for clear VoiceOver speech synthesis.
#[must_use]
pub fn format_voiceover_code(display_code: &str) -> String {
    display_code
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Candidate item staged by the iOS Share Extension after `startAccessingSecurityScopedResource()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityScopedCandidate {
    /// Path inside the shared App Group staging folder (`group.app.totaldrop.ios/SharedInbox/…`).
    pub staged_path: PathBuf,
    /// Original file name reported by `NSItemProvider` or the security-scoped URL.
    pub original_name: String,
    /// Byte length of the staged file.
    pub byte_size: u64,
    /// Whether `startAccessingSecurityScopedResource()` succeeded and the file was readable.
    pub security_scope_granted: bool,
}

/// Why an item in an iOS Share Extension batch was excluded from the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareExtensionSkipReason {
    /// Security-scoped resource access was denied by iOS sandboxing.
    SecurityScopeDenied,
    /// Staged path escaped the shared App Group directory.
    OutsideAppGroupContainer,
    /// File name violated `totaldrop_proto::validate_path` rules.
    UnsafeName,
    /// Case-insensitive duplicate of an earlier item in the same share sheet batch.
    DuplicateNameCollision,
    /// Exceeded `totaldrop_proto::limits::MAX_ENTRIES`.
    EntryLimitReached,
}

/// Validated item persisted inside an App Group [`IosShareManifest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Staged path inside the App Group container.
    pub staged_path: PathBuf,
    /// NFC-normalized safe file name.
    pub safe_name: String,
    /// Case-folded collision key.
    pub collision_key: String,
    /// Size in bytes.
    pub byte_size: u64,
    /// Expected protocol chunk count from [`totaldrop_proto::chunk_count`].
    pub chunks: u64,
}

/// Validated manifest written by the iOS Share Extension into `group.app.totaldrop.ios`
/// for handoff to the main `TotalDropIOS` process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IosShareManifest {
    /// App Group identifier (`group.app.totaldrop.ios`).
    pub app_group: String,
    /// Validated entries ready to send.
    pub entries: Vec<ManifestEntry>,
    /// Total bytes across all accepted entries.
    pub total_bytes: u64,
    /// Rejected original names and their reasons.
    pub skipped: Vec<(String, ShareExtensionSkipReason)>,
    /// Localized warning (`shareSkipped` or `shareBusy`), if any.
    pub localized_notice: Option<String>,
    /// `true` when the main app reported an active transfer and refused new items.
    pub rejected_busy: bool,
}

impl IosShareManifest {
    /// Serializes this manifest to UTF-8 JSON for atomic write into the App Group container.
    #[must_use]
    pub fn to_app_group_json(&self) -> String {
        let items: Vec<Value> = self
            .entries
            .iter()
            .map(|e| {
                json!({
                    "stagedPath": e.staged_path.to_string_lossy(),
                    "safeName": e.safe_name,
                    "collisionKey": e.collision_key,
                    "byteSize": e.byte_size,
                    "chunks": e.chunks,
                })
            })
            .collect();
        let skipped_items: Vec<Value> = self
            .skipped
            .iter()
            .map(|(original_name, reason)| {
                json!({
                    "originalName": original_name,
                    "reason": handoff::skip_reason_slug(reason),
                })
            })
            .collect();
        json!({
            "appGroup": self.app_group,
            "totalBytes": self.total_bytes,
            "skippedCount": self.skipped.len(),
            "skipped": skipped_items,
            "notice": self.localized_notice,
            "rejectedBusy": self.rejected_busy,
            "entries": items,
        })
        .to_string()
    }
}

/// Builds a validated [`IosShareManifest`] from security-scoped URLs staged into `app_group_root`.
#[must_use]
pub fn build_ios_share_manifest(
    app_group_root: &Path,
    candidates: &[SecurityScopedCandidate],
    state: MobileActivityState,
    l10n: &AppleMobileL10n,
) -> IosShareManifest {
    if !state.is_idle() {
        return IosShareManifest {
            app_group: IOS_APP_GROUP_ID.to_owned(),
            entries: Vec::new(),
            total_bytes: 0,
            skipped: Vec::new(),
            localized_notice: Some(l10n.text("shareBusy")),
            rejected_busy: true,
        };
    }

    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    let mut seen_keys = BTreeSet::new();
    let mut total_bytes: u64 = 0;

    for item in candidates {
        if entries.len() >= limits::MAX_ENTRIES {
            skipped.push((
                item.original_name.clone(),
                ShareExtensionSkipReason::EntryLimitReached,
            ));
            continue;
        }
        if !item.security_scope_granted {
            skipped.push((
                item.original_name.clone(),
                ShareExtensionSkipReason::SecurityScopeDenied,
            ));
            continue;
        }
        if !item.staged_path.starts_with(app_group_root)
            || item
                .staged_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            skipped.push((
                item.original_name.clone(),
                ShareExtensionSkipReason::OutsideAppGroupContainer,
            ));
            continue;
        }
        let Some(safe): Option<SafePath> = validate_path(&[item.original_name.as_str()]).ok()
        else {
            skipped.push((
                item.original_name.clone(),
                ShareExtensionSkipReason::UnsafeName,
            ));
            continue;
        };
        let Some(chunks) = chunk_count(item.byte_size) else {
            skipped.push((
                item.original_name.clone(),
                ShareExtensionSkipReason::UnsafeName,
            ));
            continue;
        };
        let safe_name = safe.segments().first().cloned().unwrap_or_default();
        let collision_key = safe.collision_key();
        if !seen_keys.insert(collision_key.clone()) {
            skipped.push((
                item.original_name.clone(),
                ShareExtensionSkipReason::DuplicateNameCollision,
            ));
            continue;
        }

        total_bytes = total_bytes.saturating_add(item.byte_size);
        entries.push(ManifestEntry {
            staged_path: item.staged_path.clone(),
            safe_name,
            collision_key,
            byte_size: item.byte_size,
            chunks,
        });
    }

    let localized_notice = if skipped.is_empty() {
        None
    } else {
        let count = u64::try_from(skipped.len()).unwrap_or(u64::MAX);
        Some(
            l10n.strings
                .get("shareSkipped", &[("count", Arg::Count(count))]),
        )
    };

    IosShareManifest {
        app_group: IOS_APP_GROUP_ID.to_owned(),
        entries,
        total_bytes,
        skipped,
        localized_notice,
        rejected_busy: false,
    }
}

/// Direction of an in-flight background transfer managed by `BGContinuedProcessingTask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundDirection {
    /// Sending files in the background.
    Sending,
    /// Receiving files in the background.
    Receiving,
}

/// Checkpoint metadata for an iOS/iPadOS `BGContinuedProcessingTask` or Live Activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundTransferCheckpoint {
    /// Registered `BGTaskScheduler` identifier (`app.totaldrop.ios.transfer`).
    pub task_identifier: String,
    /// Direction of transfer.
    pub direction: BackgroundDirection,
    /// Bytes verified or sent so far.
    pub done_bytes: u64,
    /// Total expected bytes across the offer.
    pub total_bytes: u64,
    /// Completed 1 MiB protocol chunks (`totaldrop_proto::limits::CHUNK_SIZE`).
    pub completed_chunks: u64,
    /// Total protocol chunks across the transfer.
    pub total_chunks: u64,
    /// Clamped integer percentage `0..=100`.
    pub percent: u8,
    /// Localized Live Activity / background notification title (`transferNoticeSending` or `transferNoticeReceiving`).
    pub notification_title: String,
    /// Localized Live Activity subtitle (`transferNoticeBody`).
    pub notification_subtitle: String,
    /// Region-formatted progress summary (`"4.2 MB / 10 MB"`).
    pub progress_summary: String,
    /// Whether iOS invoked the background task expiration handler.
    pub expired_by_system: bool,
}

impl BackgroundTransferCheckpoint {
    /// Builds a new checkpoint with localized notification text and regional byte formatting.
    #[must_use]
    pub fn capture(
        direction: BackgroundDirection,
        done_bytes: u64,
        total_bytes: u64,
        expired_by_system: bool,
        l10n: &AppleMobileL10n,
    ) -> Self {
        let clamped_done = if total_bytes > 0 {
            done_bytes.min(total_bytes)
        } else {
            done_bytes
        };
        let percent_u64 = if total_bytes == 0 {
            if done_bytes == 0 { 0 } else { 100 }
        } else {
            u128::from(clamped_done)
                .saturating_mul(100)
                .checked_div(u128::from(total_bytes))
                .and_then(|v| u64::try_from(v).ok())
                .unwrap_or(100)
                .min(100)
        };
        let percent = u8::try_from(percent_u64).unwrap_or(100);
        let completed_chunks = clamped_done.checked_div(limits::CHUNK_SIZE).unwrap_or(0);
        let total_chunks = chunk_count(total_bytes).unwrap_or(limits::MAX_CHUNKS);
        let title_key = match direction {
            BackgroundDirection::Sending => "transferNoticeSending",
            BackgroundDirection::Receiving => "transferNoticeReceiving",
        };
        let done_text = l10n.format_bytes(clamped_done);
        let total_text = l10n.format_bytes(total_bytes);

        Self {
            task_identifier: BG_TRANSFER_TASK_ID.to_owned(),
            direction,
            done_bytes: clamped_done,
            total_bytes,
            completed_chunks,
            total_chunks,
            percent,
            notification_title: l10n.text(title_key),
            notification_subtitle: l10n.text("transferNoticeBody"),
            progress_summary: format!("{done_text} / {total_text}"),
            expired_by_system,
        }
    }

    /// Serializes checkpoint metadata to JSON for App Group inspection on relaunch.
    #[must_use]
    pub fn to_json(&self) -> String {
        let dir = match self.direction {
            BackgroundDirection::Sending => "sending",
            BackgroundDirection::Receiving => "receiving",
        };
        json!({
            "taskIdentifier": self.task_identifier,
            "direction": dir,
            "doneBytes": self.done_bytes,
            "totalBytes": self.total_bytes,
            "completedChunks": self.completed_chunks,
            "totalChunks": self.total_chunks,
            "percent": self.percent,
            "notificationTitle": self.notification_title,
            "notificationSubtitle": self.notification_subtitle,
            "progressSummary": self.progress_summary,
            "expiredBySystem": self.expired_by_system,
        })
        .to_string()
    }
}

/// Directional D-pad / swipe input from the Apple TV Siri Remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiriRemoteDirection {
    /// Up on the Siri Remote clickpad.
    Up,
    /// Down on the Siri Remote clickpad.
    Down,
    /// Left on the Siri Remote clickpad (flipped automatically when `is_rtl` is `true`).
    Left,
    /// Right on the Siri Remote clickpad (flipped automatically when `is_rtl` is `true`).
    Right,
}

/// Focusable cards in the Apple tvOS 10-foot pairing & verification interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TvFocusCard {
    /// Large monospace transfer code card (`7-harbor-rocket`).
    CodeBanner,
    /// High-contrast QR code card scannable from a couch distance (3 meters).
    QrCodeCard,
    /// Four verification words card (`verificationTitle`).
    VerificationCard,
    /// Primary confirmation button (`Accept` or `New Code`).
    PrimaryAction,
    /// Secondary cancel/decline button (`Decline` or `Stop`).
    CancelAction,
}

/// 10-foot living-room pairing session model for Apple tvOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvPairingSession {
    /// Active transfer code (`7-harbor-rocket`).
    pub display_code: String,
    /// Canonical numeric code for CPace (`7-840-1499`).
    pub canonical_code: String,
    /// Universal Link encoded into the 10-foot TV QR card.
    pub qr_payload: String,
    /// Four verification words once Noise + ML-KEM-768 connects.
    pub verification_words: Vec<String>,
    /// Currently focused card under the tvOS SwiftUI Focus Engine.
    pub focused_card: TvFocusCard,
    /// Whether the active language uses right-to-left horizontal card order.
    pub is_rtl: bool,
    /// Ephemeral tvOS `NSCachesDirectory/TotalDrop` staging folder.
    pub caches_staging_dir: PathBuf,
}

impl TvPairingSession {
    /// Creates a new tvOS 10-foot pairing session around `code` and `caches_root`.
    #[must_use]
    pub fn new(code: &TransferCode, caches_root: &Path, is_rtl: bool) -> Self {
        let display_code = code.to_english();
        let canonical_code = code.canonical();
        let qr_payload = format!("{UNIVERSAL_LINK_PREFIX}{display_code}");
        Self {
            display_code,
            canonical_code,
            qr_payload,
            verification_words: Vec::new(),
            focused_card: TvFocusCard::QrCodeCard,
            is_rtl,
            caches_staging_dir: caches_root.join("TotalDrop"),
        }
    }

    /// Attaches the 4 verification words once the encrypted session connects
    /// and shifts initial focus to the verification card for user review.
    pub fn set_connected_words(&mut self, words: Vec<String>) {
        self.verification_words = words;
        self.focused_card = TvFocusCard::VerificationCard;
    }

    /// Computes the next focused tvOS card when the user presses `direction`
    /// on the Siri Remote, respecting RTL horizontal mirroring.
    pub fn move_focus(&mut self, direction: SiriRemoteDirection) -> TvFocusCard {
        let effective = if self.is_rtl {
            match direction {
                SiriRemoteDirection::Left => SiriRemoteDirection::Right,
                SiriRemoteDirection::Right => SiriRemoteDirection::Left,
                other => other,
            }
        } else {
            direction
        };

        self.focused_card = match (self.focused_card, effective) {
            (TvFocusCard::CodeBanner, SiriRemoteDirection::Right) => TvFocusCard::QrCodeCard,
            (TvFocusCard::QrCodeCard, SiriRemoteDirection::Left) => TvFocusCard::CodeBanner,
            (TvFocusCard::CodeBanner | TvFocusCard::QrCodeCard, SiriRemoteDirection::Down) => {
                if self.verification_words.is_empty() {
                    TvFocusCard::CancelAction
                } else {
                    TvFocusCard::VerificationCard
                }
            }
            (TvFocusCard::VerificationCard, SiriRemoteDirection::Up) => TvFocusCard::QrCodeCard,
            (TvFocusCard::VerificationCard, SiriRemoteDirection::Down) => {
                TvFocusCard::PrimaryAction
            }
            (TvFocusCard::PrimaryAction, SiriRemoteDirection::Right) => TvFocusCard::CancelAction,
            (TvFocusCard::CancelAction, SiriRemoteDirection::Left) => {
                if self.verification_words.is_empty() {
                    TvFocusCard::CodeBanner
                } else {
                    TvFocusCard::PrimaryAction
                }
            }
            (TvFocusCard::PrimaryAction | TvFocusCard::CancelAction, SiriRemoteDirection::Up) => {
                if self.verification_words.is_empty() {
                    TvFocusCard::QrCodeCard
                } else {
                    TvFocusCard::VerificationCard
                }
            }
            (current, _) => current,
        };
        self.focused_card
    }
}

/// UIKit / SwiftUI `DynamicTypeSize` categories from `xSmall` through `accessibility5`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DynamicTypeScale {
    /// `xSmall` through `xxxLarge` standard text sizes.
    Standard,
    /// `accessibility1` (`AX1`).
    Accessibility1,
    /// `accessibility2` (`AX2`).
    Accessibility2,
    /// `accessibility3` (`AX3`).
    Accessibility3,
    /// `accessibility4` (`AX4`).
    Accessibility4,
    /// `accessibility5` (`AX5`, largest iOS accessibility text scale).
    Accessibility5,
}

impl DynamicTypeScale {
    /// Parses a SwiftUI `DynamicTypeSize` identifier string into [`DynamicTypeScale`].
    #[must_use]
    pub fn from_apple_name(name: &str) -> Self {
        match name.trim() {
            "accessibility1" | "AX1" => Self::Accessibility1,
            "accessibility2" | "AX2" => Self::Accessibility2,
            "accessibility3" | "AX3" => Self::Accessibility3,
            "accessibility4" | "AX4" => Self::Accessibility4,
            "accessibility5" | "AX5" => Self::Accessibility5,
            _ => Self::Standard,
        }
    }

    /// Whether side-by-side controls (`HStack`) must switch to vertical stacking (`VStack`)
    /// so text never truncates at `accessibility1`..=`accessibility5`.
    #[must_use]
    pub const fn prefers_vertical_stack(self) -> bool {
        !matches!(self, Self::Standard)
    }

    /// Minimum touch target height in points (`44pt` standard HIG, `60pt+` at AX scales).
    #[must_use]
    pub const fn min_tap_height_pt(self) -> u16 {
        match self {
            Self::Standard => 44,
            Self::Accessibility1 | Self::Accessibility2 => 56,
            Self::Accessibility3 | Self::Accessibility4 | Self::Accessibility5 => 68,
        }
    }
}

/// A localized UI string paired with Dynamic Type and VoiceOver layout metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicTypeSafeString {
    /// Resolved localized text from `totaldrop-l10n`.
    pub text: String,
    /// Whether the view should stack controls vertically at `scale`.
    pub stack_vertically: bool,
    /// Whether `lineLimit(nil)` (unlimited wrapping) is required so nothing clips.
    pub unlimited_lines: bool,
    /// Minimum accessible tap height in points.
    pub min_tap_height_pt: u16,
    /// Whether the language requires right-to-left layout direction.
    pub is_rtl: bool,
}

/// 35-language localization and Dynamic Type bridge for iOS, iPadOS, and tvOS.
#[derive(Debug)]
pub struct AppleMobileL10n {
    strings: Strings,
}

impl AppleMobileL10n {
    /// Negotiates the best supported language from Apple `Locale.preferredLanguages`
    /// and an optional regional number locale (`Locale.current.identifier`).
    #[must_use]
    pub fn from_apple_preferences(
        preferred_languages: &[String],
        numeric_locale: Option<&str>,
    ) -> Self {
        let prefs: Vec<_> = preferred_languages
            .iter()
            .filter_map(|t| totaldrop_l10n::locale::parse(&clean_apple_locale_tag(t)))
            .collect();
        let numeric = numeric_locale
            .map(clean_apple_locale_tag)
            .and_then(|t| totaldrop_l10n::locale::parse(&t));
        Self {
            strings: Strings::new(Domain::App, &prefs, numeric.as_ref()),
        }
    }

    /// All supported ARB locale tags compiled into `totaldrop-l10n` (`Domain::App`).
    #[must_use]
    pub fn supported_locales() -> Vec<String> {
        totaldrop_l10n::available(Domain::App)
    }

    /// Active BCP-47 tag (for example `"en"`, `"tr"`, `"ar"`, `"zh-Hans"`, `"zh-Hant"`).
    #[must_use]
    pub fn tag(&self) -> &str {
        self.strings.tag()
    }

    /// Whether views should apply right-to-left layout direction.
    #[must_use]
    pub fn is_rtl(&self) -> bool {
        self.strings.is_rtl()
    }

    /// Whether `key` is a safety-critical string whose translation is an unreviewed draft.
    #[must_use]
    pub fn key_needs_english(&self, key: &str) -> bool {
        totaldrop_l10n::needs_english(self.strings.tag(), key)
    }

    /// Looks up a parameterless localized string.
    #[must_use]
    pub fn text(&self, key: &str) -> String {
        self.strings.get(key, &[])
    }

    /// Resolves `key` together with Dynamic Type layout directives up to `accessibility5`.
    #[must_use]
    pub fn dynamic_text(&self, key: &str, scale: DynamicTypeScale) -> DynamicTypeSafeString {
        DynamicTypeSafeString {
            text: self.text(key),
            stack_vertically: scale.prefers_vertical_stack(),
            unlimited_lines: true,
            min_tap_height_pt: scale.min_tap_height_pt(),
            is_rtl: self.is_rtl(),
        }
    }

    /// Formats a byte count using regional decimal and unit conventions (`"1.5 MB"` / `"1,5 MB"`).
    #[must_use]
    pub fn format_bytes(&self, bytes: u64) -> String {
        self.strings.bytes(bytes)
    }

    /// Sanitizes peer-supplied file names by revealing hidden bidi/control characters (`<U+202E>`).
    #[must_use]
    pub fn sanitize_name(&self, raw_name: &str) -> String {
        totaldrop_l10n::reveal_controls(raw_name)
    }

    /// Sanitizes peer-supplied text and caps inline length to [`MAX_INLINE_TEXT_CHARS`].
    #[must_use]
    pub fn sanitize_received_text(&self, raw: &str) -> (String, bool, bool) {
        let revealed = totaldrop_l10n::reveal_text(raw);
        let had_hidden = revealed != raw.replace("\r\n", "\n");
        match revealed.char_indices().nth(MAX_INLINE_TEXT_CHARS) {
            Some((idx, _)) => (
                revealed.get(..idx).unwrap_or_default().to_owned(),
                true,
                had_hidden,
            ),
            None => (revealed, false, had_hidden),
        }
    }
}

fn clean_apple_locale_tag(raw: &str) -> String {
    let without_calendar = raw.split('@').next().unwrap_or(raw);
    without_calendar.trim().replace('_', "-")
}

/// Thread-safe transfer cancellation handle connected to iOS `BGTask.expirationHandler`
/// and tvOS Siri Remote `Menu`/`Exit` actions.
#[derive(Clone, Default)]
pub struct MobileStopToken {
    stop: Stop,
}

impl std::fmt::Debug for MobileStopToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobileStopToken")
            .field("stopped", &self.stop.is_stopped())
            .finish()
    }
}

impl MobileStopToken {
    /// Creates an active, unstopped cancellation token.
    #[must_use]
    pub fn new() -> Self {
        Self { stop: Stop::new() }
    }

    /// Cancels the transfer (invoked on user Stop or `BGTask` expiration).
    pub fn cancel(&self) {
        self.stop.stop();
    }

    /// Whether cancellation has been triggered.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.stop.is_stopped()
    }

    /// Underlying [`totaldrop_core::Stop`] reference.
    #[must_use]
    pub const fn as_core_stop(&self) -> &Stop {
        &self.stop
    }
}

/// Network and privacy settings shared across iOS, iPadOS, and tvOS.
#[derive(Debug, Clone)]
pub struct MobileTransferConfig {
    /// Rendezvous WebSocket URL.
    pub server: String,
    /// Relay WebSocket URL.
    pub relay: String,
    /// STUN server (`host:port`).
    pub stun: String,
    /// Privacy Mode (`relay_only`).
    pub privacy_mode: bool,
    /// 3-word stronger code toggle.
    pub stronger_code: bool,
}

impl Default for MobileTransferConfig {
    fn default() -> Self {
        let paths = PathOptions::default();
        Self {
            server: totaldrop_core::rendezvous::DEFAULT_SERVER.to_owned(),
            relay: paths.relay,
            stun: paths.stun,
            privacy_mode: false,
            stronger_code: false,
        }
    }
}

impl MobileTransferConfig {
    /// Builds [`totaldrop_core::SendOptions`].
    #[must_use]
    pub fn send_options(&self) -> SendOptions {
        SendOptions {
            server: self.server.clone(),
            words: if self.stronger_code { 3 } else { 2 },
            paths: self.path_options(),
        }
    }

    /// Builds [`totaldrop_core::ReceiveOptions`].
    #[must_use]
    pub fn receive_options(&self) -> ReceiveOptions {
        ReceiveOptions {
            server: self.server.clone(),
            paths: self.path_options(),
        }
    }

    fn path_options(&self) -> PathOptions {
        PathOptions {
            relay: self.relay.clone(),
            stun: self.stun.clone(),
            relay_only: self.privacy_mode,
            gateway: if self.privacy_mode {
                totaldrop_core::portmap::Gateway::Off
            } else {
                totaldrop_core::portmap::Gateway::Auto
            },
        }
    }
}

/// Maps [`totaldrop_core::Error`] into localized headline and plain-language cause strings.
#[must_use]
pub fn localize_mobile_error(
    err: &CoreError,
    is_sender: bool,
    l10n: &AppleMobileL10n,
) -> (String, Option<String>, String) {
    let key = match err {
        CoreError::WrongCode => "errorWrongCode",
        CoreError::PeerLeft => "errorPeerLeft",
        CoreError::Rejected(_) => "errorDeclined",
        CoreError::Cancelled(_) => "errorCancelled",
        CoreError::Stopped => "errorStopped",
        CoreError::Code(_) => "errorInvalidCode",
        CoreError::Rendezvous(_) if is_sender && err.is_code_not_in_use() => "errorCodeExpired",
        CoreError::Rendezvous(_) if !is_sender && err.is_code_not_in_use() => "errorCodeNotInUse",
        CoreError::Rendezvous(_) if err.is_bad_server_address() => "errorBadServer",
        CoreError::Rendezvous(_) => "errorServer",
        CoreError::Connect(_) => "errorConnect",
        CoreError::InvalidOffer(_) => "errorInvalidOffer",
        CoreError::Io(_) => "errorStorage",
        CoreError::Protocol(_) | CoreError::Random => "errorOther",
    };
    let cause_id = err.cause().as_str();
    let cause = if cause_id.is_empty() {
        None
    } else {
        Some(l10n.text(&format!("cause{cause_id}")))
    };
    (l10n.text(key), cause, l10n.sanitize_name(&err.to_string()))
}

/// Prepares an iOS/tvOS file send via [`totaldrop_core::PreparedSend::new`].
///
/// # Errors
///
/// Returns an error if file scanning fails, the rendezvous server is unreachable,
/// or `token` cancels preparation.
pub async fn prepare_mobile_send(
    config: &MobileTransferConfig,
    staged_files: Vec<PathBuf>,
    token: &MobileStopToken,
) -> totaldrop_core::Result<PreparedSend> {
    if token.is_cancelled() {
        return Err(CoreError::Stopped);
    }
    let opts = config.send_options();
    tokio::select! {
        biased;
        res = PreparedSend::new(&opts, staged_files) => res,
        () = token.as_core_stop().stopped() => Err(CoreError::Stopped),
    }
}

/// Connects to a sender's mailbox on iOS, iPadOS, or tvOS via [`totaldrop_core::IncomingOffer::connect_with`].
///
/// # Errors
///
/// Returns an error if `code_or_link` is invalid, CPace fails, or `token` cancels connection.
pub async fn connect_mobile_receive(
    config: &MobileTransferConfig,
    code_or_link: &str,
    token: &MobileStopToken,
) -> totaldrop_core::Result<IncomingOffer> {
    if token.is_cancelled() {
        return Err(CoreError::Stopped);
    }
    let code = TransferCode::from_link(code_or_link)
        .ok_or(CoreError::Code(totaldrop_proto::CodeError::BadNameplate))?;
    IncomingOffer::connect_with(
        &config.receive_options(),
        &code.to_english(),
        token.as_core_stop(),
    )
    .await
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn en_l10n() -> AppleMobileL10n {
        AppleMobileL10n::from_apple_preferences(&["en-US".to_owned()], Some("en_US"))
    }

    #[test]
    fn universal_link_parser_extracts_fragment_code_and_voiceover_cues() {
        let l10n = en_l10n();
        let route = parse_universal_link(
            "https://totaldrop.app/r#7-harbor-rocket",
            MobileActivityState::Idle,
            &l10n,
        );
        assert_eq!(
            route,
            UniversalLinkRoute::ConnectReceive {
                display_code: "7-harbor-rocket".to_owned(),
                canonical_code: TransferCode::parse("7-harbor-rocket").unwrap().canonical(),
                universal_link: "https://totaldrop.app/r#7-harbor-rocket".to_owned(),
                voiceover_spoken_code: "7, harbor, rocket".to_owned(),
            }
        );
    }

    #[test]
    fn universal_link_parser_accepts_arabic_indic_digits_and_custom_scheme() {
        let l10n = AppleMobileL10n::from_apple_preferences(&["ar-SA".to_owned()], None);
        let route = parse_universal_link(
            "totaldrop://r/\u{0667}-harbor-rocket",
            MobileActivityState::Idle,
            &l10n,
        );
        match route {
            UniversalLinkRoute::ConnectReceive { display_code, .. } => {
                assert_eq!(display_code, "7-harbor-rocket");
            }
            other => panic!("expected ConnectReceive, got {other:?}"),
        }
    }

    #[test]
    fn universal_link_parser_guards_active_background_transfer_and_invalid_links() {
        let l10n = en_l10n();
        let busy = parse_universal_link(
            "https://totaldrop.app/r#7-harbor-rocket",
            MobileActivityState::ActiveSend,
            &l10n,
        );
        assert!(matches!(busy, UniversalLinkRoute::Busy { .. }));
        assert_eq!(
            parse_universal_link(
                "https://phishing.example/r#7-harbor-rocket",
                MobileActivityState::Idle,
                &l10n,
            ),
            UniversalLinkRoute::Invalid
        );
    }

    #[test]
    fn ios_share_manifest_normalizes_nfd_and_enforces_app_group_containment() {
        let l10n = en_l10n();
        let group_root =
            Path::new("/private/var/mobile/Containers/Shared/AppGroup/UUID/SharedInbox");
        let candidates = vec![
            SecurityScopedCandidate {
                staged_path: group_root.join("1/Vacances_e\u{0301}te\u{0301}.heic"),
                original_name: "Vacances_e\u{0301}te\u{0301}.heic".to_owned(),
                byte_size: 2_500_000,
                security_scope_granted: true,
            },
            SecurityScopedCandidate {
                staged_path: PathBuf::from("/private/var/mobile/Library/SMS/secret.db"),
                original_name: "secret.db".to_owned(),
                byte_size: 4096,
                security_scope_granted: true,
            },
            SecurityScopedCandidate {
                staged_path: group_root.join("2/locked.pdf"),
                original_name: "locked.pdf".to_owned(),
                byte_size: 1024,
                security_scope_granted: false,
            },
        ];

        let manifest =
            build_ios_share_manifest(group_root, &candidates, MobileActivityState::Idle, &l10n);
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.entries[0].safe_name, "Vacances_été.heic");
        assert_eq!(manifest.entries[0].chunks, 3);
        assert_eq!(manifest.total_bytes, 2_500_000);
        assert_eq!(manifest.skipped.len(), 2);
        assert_eq!(
            manifest.skipped[0].1,
            ShareExtensionSkipReason::OutsideAppGroupContainer
        );
        assert_eq!(
            manifest.skipped[1].1,
            ShareExtensionSkipReason::SecurityScopeDenied
        );
        assert!(manifest.localized_notice.is_some());

        let parsed: Value = serde_json::from_str(&manifest.to_app_group_json()).unwrap();
        assert_eq!(parsed["appGroup"], IOS_APP_GROUP_ID);
        assert_eq!(parsed["skippedCount"], 2);
    }

    #[test]
    fn ios_share_manifest_rejects_case_collisions_and_busy_main_app() {
        let l10n = en_l10n();
        let group_root = Path::new("/AppGroup/Inbox");
        let candidates = vec![
            SecurityScopedCandidate {
                staged_path: group_root.join("a/IMG_0001.JPG"),
                original_name: "IMG_0001.JPG".to_owned(),
                byte_size: 100,
                security_scope_granted: true,
            },
            SecurityScopedCandidate {
                staged_path: group_root.join("b/img_0001.jpg"),
                original_name: "img_0001.jpg".to_owned(),
                byte_size: 200,
                security_scope_granted: true,
            },
        ];
        let manifest =
            build_ios_share_manifest(group_root, &candidates, MobileActivityState::Idle, &l10n);
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(
            manifest.skipped[0].1,
            ShareExtensionSkipReason::DuplicateNameCollision
        );

        let busy = build_ios_share_manifest(
            group_root,
            &candidates,
            MobileActivityState::ActiveReceive,
            &l10n,
        );
        assert!(busy.rejected_busy);
        assert!(busy.entries.is_empty());
    }

    #[test]
    fn background_checkpoint_computes_chunks_and_localized_live_activity_labels() {
        let fr = AppleMobileL10n::from_apple_preferences(&["fr-FR".to_owned()], Some("fr_FR"));
        let chunk = limits::CHUNK_SIZE;
        let cp = BackgroundTransferCheckpoint::capture(
            BackgroundDirection::Sending,
            chunk.saturating_mul(2),
            chunk.saturating_mul(4),
            false,
            &fr,
        );
        assert_eq!(cp.task_identifier, BG_TRANSFER_TASK_ID);
        assert_eq!(cp.percent, 50);
        assert_eq!(cp.completed_chunks, 2);
        assert_eq!(cp.total_chunks, 4);
        assert!(!cp.notification_title.is_empty());
        assert_ne!(cp.notification_title, "transferNoticeSending");

        let json_val: Value = serde_json::from_str(&cp.to_json()).unwrap();
        assert_eq!(json_val["percent"], 50);
        assert_eq!(json_val["direction"], "sending");
    }

    #[test]
    fn tvos_10_foot_pairing_navigates_siri_remote_focus_in_ltr_and_rtl() {
        let code = TransferCode::parse("7-harbor-rocket").unwrap();
        let caches = Path::new("/Library/Caches");
        let mut ltr = TvPairingSession::new(&code, caches, false);
        assert_eq!(ltr.focused_card, TvFocusCard::QrCodeCard);
        assert_eq!(ltr.qr_payload, "https://totaldrop.app/r#7-harbor-rocket");
        assert_eq!(
            ltr.caches_staging_dir,
            PathBuf::from("/Library/Caches/TotalDrop")
        );

        assert_eq!(
            ltr.move_focus(SiriRemoteDirection::Left),
            TvFocusCard::CodeBanner
        );
        ltr.set_connected_words(vec![
            "able".into(),
            "acid".into(),
            "aged".into(),
            "also".into(),
        ]);
        assert_eq!(ltr.focused_card, TvFocusCard::VerificationCard);
        assert_eq!(
            ltr.move_focus(SiriRemoteDirection::Down),
            TvFocusCard::PrimaryAction
        );
        assert_eq!(
            ltr.move_focus(SiriRemoteDirection::Right),
            TvFocusCard::CancelAction
        );

        // In RTL (e.g. Arabic or Hebrew on Apple TV), Left moves toward CancelAction.
        let mut rtl = TvPairingSession::new(&code, caches, true);
        rtl.set_connected_words(vec!["w1".into(), "w2".into(), "w3".into(), "w4".into()]);
        assert_eq!(
            rtl.move_focus(SiriRemoteDirection::Down),
            TvFocusCard::PrimaryAction
        );
        assert_eq!(
            rtl.move_focus(SiriRemoteDirection::Left),
            TvFocusCard::CancelAction
        );
    }

    #[test]
    fn all_35_plus_languages_and_dynamic_type_accessibility5_are_supported() {
        let locales = AppleMobileL10n::supported_locales();
        assert!(locales.len() >= 35);

        let hebrew = AppleMobileL10n::from_apple_preferences(&["he-IL".to_owned()], None);
        assert_eq!(hebrew.tag(), "he");
        assert!(hebrew.is_rtl());

        let ax5 = DynamicTypeScale::from_apple_name("accessibility5");
        assert_eq!(ax5, DynamicTypeScale::Accessibility5);
        let send_title = hebrew.dynamic_text("sendTitle", ax5);
        assert!(send_title.stack_vertically);
        assert!(send_title.unlimited_lines);
        assert_eq!(send_title.min_tap_height_pt, 68);
        assert!(send_title.is_rtl);

        let std_scale = DynamicTypeScale::from_apple_name("large");
        assert!(!std_scale.prefers_vertical_stack());
        assert_eq!(std_scale.min_tap_height_pt(), 44);
    }

    #[test]
    fn hidden_bidi_controls_and_long_received_texts_are_sanitized() {
        let l10n = en_l10n();
        assert_eq!(
            l10n.sanitize_name("photo\u{202E}gnp.app"),
            "photo<U+202E>gnp.app"
        );
        let (shortened, was_cut, had_hidden) = l10n.sanitize_received_text(&format!(
            "\u{2066}{}",
            "x".repeat(MAX_INLINE_TEXT_CHARS + 10)
        ));
        assert!(was_cut);
        assert!(had_hidden);
        assert_eq!(shortened.chars().count(), MAX_INLINE_TEXT_CHARS);
    }

    #[test]
    fn mobile_stop_token_and_privacy_mode_options_propagate_cleanly() {
        let l10n = en_l10n();
        let cfg = MobileTransferConfig {
            privacy_mode: true,
            stronger_code: true,
            ..Default::default()
        };
        let opts = cfg.send_options();
        assert_eq!(opts.words, 3);
        assert!(opts.paths.relay_only);

        let token = MobileStopToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());

        let (headline, cause, _) = localize_mobile_error(&CoreError::PeerLeft, true, &l10n);
        assert!(headline.contains("left"));
        assert!(cause.is_none());
    }
}
