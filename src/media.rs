//! Attachments carried by a `send` request: the wire mirror (`SendFile`), the
//! validated form the client is handed (`MediaFile`), and the one validator
//! both go through.
//!
//! Two facts shape everything here. First, **no TDLib `Input*` type carries a
//! filename or a MIME type** — the recipient is shown the basename of the local
//! path — so the caller is responsible for materialising each file under the
//! name it should arrive as, and `tg` renames nothing. Second, TDLib groups
//! only same-typed contents into an album, so a request that mixes a photo with
//! a document has no single-message rendering and is refused rather than
//! silently split into two messages. A voice note goes in no album at all, so
//! it is always the request's one file.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::error::{Result, TgError};

/// TDLib albums carry 2-10 contents; a single file is sent as one message.
pub const MAX_FILES: usize = 10;

/// A voice note's waveform as the caller passes it: this many levels, oldest
/// first, each `0..=`[`WAVEFORM_MAX`].
pub const WAVEFORM_LEVELS: usize = 64;

/// The loudest level a caller passes; Telegram's own scale tops out at 31.
pub const WAVEFORM_MAX: i64 = 100;

/// Wire mirror of one `args.files[]` entry. `deny_unknown_fields` for the same
/// reason [`crate::commands::send::SendRequest`] carries it: a dropped key here
/// means a file delivered with the wrong presentation and `ok:true`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SendFile {
    /// Absolute path to the file. Its basename is what the recipient sees.
    pub path: String,
    /// `photo`, `file` or `voice`; absent means `file`. Kept as a string so
    /// this struct stays a transparent wire mirror and the error text stays
    /// ours.
    #[serde(default)]
    pub kind: Option<String>,
    /// Photo pixel width; `0` or absent lets TDLib work it out.
    #[serde(default)]
    pub width: Option<i32>,
    /// Photo pixel height; `0` or absent lets TDLib work it out.
    #[serde(default)]
    pub height: Option<i32>,
    /// A voice note's length in whole seconds, 1 or more. Required for kind
    /// `voice`, refused for any other. Wider than TDLib's `int32` so an
    /// out-of-range value is refused in our words, not serde's.
    #[serde(default)]
    pub duration: Option<i64>,
    /// A voice note's waveform: exactly [`WAVEFORM_LEVELS`] levels, oldest
    /// first, each `0..=100`. Required for kind `voice`, refused for any other.
    #[serde(default)]
    pub waveform: Option<Vec<i64>>,
}

/// How a file is presented to the recipient. Decided by the caller and never
/// re-derived from the bytes: it is what the message *looks like*, so guessing
/// would change a payload a human already approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// An inline image.
    Photo,
    /// A document — an attachment with a filename, whatever its content type.
    File,
    /// A voice note: an Ogg Opus recording shown with a play button, its
    /// length and its waveform. Always sent alone.
    Voice,
}

impl MediaKind {
    /// Strict, case-sensitive, absent means `file`. An unrecognised value is a
    /// caller bug and is refused rather than downgraded, because the fallback
    /// would change how the recipient sees the message.
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value {
            None | Some("file") => Ok(Self::File),
            Some("photo") => Ok(Self::Photo),
            Some("voice") => Ok(Self::Voice),
            Some(other) => Err(TgError::Other(format!(
                "kind '{other}' is not supported. Expected `photo`, `file` or `voice`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::File => "file",
            Self::Voice => "voice",
        }
    }
}

/// What the client builds for one file, with exactly the measures its kind
/// reads: a photo's dimensions, a voice note's length and waveform, nothing
/// for a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presentation {
    /// `0` for a dimension lets TDLib work it out.
    Photo {
        width: i32,
        height: i32,
    },
    File,
    Voice(VoiceNote),
}

impl Presentation {
    pub fn kind(&self) -> MediaKind {
        match self {
            Self::Photo { .. } => MediaKind::Photo,
            Self::File => MediaKind::File,
            Self::Voice(_) => MediaKind::Voice,
        }
    }
}

/// A voice note's measures, as the caller passed them. `tg` neither probes
/// nor transcodes the recording: there is no decoder in its image, and the
/// caller already measured what it recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceNote {
    /// Whole seconds, 1 or more.
    pub duration: i32,
    /// [`WAVEFORM_LEVELS`] levels, oldest first, each `0..=100`.
    pub levels: Vec<u8>,
}

impl VoiceNote {
    /// The waveform as `inputMessageVoiceNote.waveform` takes it: Telegram's
    /// 5-bit form ([`pack_waveform`]), in base64 because TDLib's JSON interface
    /// carries every `bytes` field so.
    pub fn tdlib_waveform(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(pack_waveform(&self.levels))
    }
}

/// One level `0..=100` on Telegram's waveform scale `0..=31`, rounded to the
/// nearest step.
pub fn five_bit(level: u8) -> u8 {
    ((u16::from(level) * 31 + 50) / 100) as u8
}

/// Pack levels `0..=100` into Telegram's 5-bit waveform: each level, scaled by
/// [`five_bit`], takes the next 5 bits of a little-endian bit stream, the
/// first level in the lowest bits of the first byte. That is how Telegram's
/// clients write the voice note attribute's waveform and how they read it
/// back (tdesktop's `documentWaveformEncode5bit`, Android's `getWaveform2`), so
/// 64 levels are 40 bytes.
pub fn pack_waveform(levels: &[u8]) -> Vec<u8> {
    let mut packed = vec![0u8; (levels.len() * 5).div_ceil(8)];
    for (i, &level) in levels.iter().enumerate() {
        let bit = i * 5;
        let spread = u16::from(five_bit(level)) << (bit % 8);
        packed[bit / 8] |= spread as u8;
        // A value that crosses a byte boundary ends inside the vector: the
        // last level's bits end at bit `len * 5 - 1`, which the length rounds
        // up to cover.
        if spread > 0xFF {
            packed[bit / 8 + 1] |= (spread >> 8) as u8;
        }
    }
    packed
}

/// A validated attachment: an existing, readable, absolute path plus the
/// presentation the caller asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFile {
    pub path: PathBuf,
    pub presentation: Presentation,
}

impl MediaFile {
    pub fn kind(&self) -> MediaKind {
        self.presentation.kind()
    }

    /// The basename the recipient will see. Empty only for a path that
    /// validation already refused.
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Validate an optional `files` array into the set the client is handed.
///
/// `None` (the key absent) is a text-only send and yields an empty vec — that
/// is the back-compatible path and must stay free of every check below. An
/// empty *array* is refused instead of being treated as text: it is a caller
/// bug either way, and re-reading it as a plain send would deliver a message
/// the caller did not describe.
///
/// Every check here is pure request shape plus a `stat`, and it runs before the
/// recipient ladder, so a malformed request costs no TDLib round trip and
/// nothing is sent.
pub fn validate_files(files: Option<&[SendFile]>) -> Result<Vec<MediaFile>> {
    let Some(files) = files else {
        return Ok(Vec::new());
    };

    if files.is_empty() {
        return Err(TgError::Other(
            "send: `files` is present but empty; omit the key to send a text-only message"
                .to_string(),
        ));
    }

    if files.len() > MAX_FILES {
        return Err(TgError::Other(format!(
            "send: {} files requested; a Telegram album carries at most {MAX_FILES} — split the message",
            files.len()
        )));
    }

    // Shape first, filesystem second: the kind and measure faults cost
    // nothing to find, and the voice-alone and mixed-kind refusals are about
    // the request as a whole, so they must be reported before any single path
    // is blamed.
    let presentations = files
        .iter()
        .enumerate()
        .map(|(i, file)| presentation(i, file))
        .collect::<Result<Vec<_>>>()?;
    let kinds: Vec<MediaKind> = presentations.iter().map(Presentation::kind).collect();

    check_voice_alone(&kinds)?;

    if let Some(kind) = kinds.first()
        && let Some(other) = kinds.iter().position(|k| k != kind)
    {
        return Err(TgError::Other(format!(
            "send: files mixes kind `{}` (files[0]) with kind `{}` (files[{other}]); TDLib groups only same-typed contents into one message — split it into two sends",
            kind.as_str(),
            kinds[other].as_str()
        )));
    }

    files
        .iter()
        .zip(presentations)
        .enumerate()
        .map(|(i, (file, presentation))| {
            Ok(MediaFile {
                path: validate_path(i, &file.path)?,
                presentation,
            })
        })
        .collect()
}

/// A voice note is never part of an album: Telegram groups photos, videos,
/// documents and audio files, and a voice note in a several-file request has
/// no single-message rendering. Refused, never split, as a mixed request is.
/// Shared with the client's own belt-and-braces check.
pub fn check_voice_alone(kinds: &[MediaKind]) -> Result<()> {
    if kinds.len() < 2 {
        return Ok(());
    }
    match kinds.iter().position(|k| *k == MediaKind::Voice) {
        Some(voice) => Err(TgError::Other(format!(
            "send: files[{voice}] is kind `voice` and the request has {} files; a voice note is sent alone (Telegram puts none in an album) — send it by itself",
            kinds.len()
        ))),
        None => Ok(()),
    }
}

/// One entry's kind with exactly the measures it reads, every other measure
/// refused rather than ignored: a measure on the wrong kind says something
/// about the payload that kind cannot honour, and a dropped field is how a
/// caller ends up believing it asked for another presentation.
fn presentation(i: usize, file: &SendFile) -> Result<Presentation> {
    let kind = MediaKind::parse(file.kind.as_deref())
        .map_err(|e| TgError::Other(format!("send: files[{i}]: {e}")))?;
    validate_dimensions(i, file, kind)?;
    if kind != MediaKind::Voice {
        for (name, set) in [
            ("duration", file.duration.is_some()),
            ("waveform", file.waveform.is_some()),
        ] {
            if set {
                return Err(TgError::Other(format!(
                    "send: files[{i}].{name} is set but kind is `{}`; duration and waveform are read only for kind `voice`",
                    kind.as_str()
                )));
            }
        }
    }
    Ok(match kind {
        MediaKind::Photo => Presentation::Photo {
            width: file.width.unwrap_or(0),
            height: file.height.unwrap_or(0),
        },
        MediaKind::File => Presentation::File,
        MediaKind::Voice => Presentation::Voice(voice_note(i, file)?),
    })
}

fn validate_dimensions(i: usize, file: &SendFile, kind: MediaKind) -> Result<()> {
    for (name, value) in [("width", file.width), ("height", file.height)] {
        let Some(value) = value else { continue };
        if value < 0 {
            return Err(TgError::Other(format!(
                "send: files[{i}].{name} is {value}; pixel dimensions cannot be negative (use 0 to let Telegram work it out)"
            )));
        }
        // A voice note takes no dimension at all, not even the `0` a document
        // tolerates: there is nothing for one to describe.
        if (value != 0 && kind != MediaKind::Photo) || kind == MediaKind::Voice {
            return Err(TgError::Other(format!(
                "send: files[{i}].{name} is set but kind is `{}`; dimensions are read only for kind `photo`",
                kind.as_str()
            )));
        }
    }
    Ok(())
}

/// A voice note's length and waveform, both required: without them Telegram
/// shows `0:00` and a flat line, a voice note nobody approved.
fn voice_note(i: usize, file: &SendFile) -> Result<VoiceNote> {
    let duration = match file.duration {
        None => {
            return Err(TgError::Other(format!(
                "send: files[{i}].duration is missing; kind `voice` needs its length in whole seconds (1 or more)"
            )));
        }
        Some(seconds) if seconds < 1 => {
            return Err(TgError::Other(format!(
                "send: files[{i}].duration is {seconds}; a voice note lasts 1 second or more"
            )));
        }
        Some(seconds) => i32::try_from(seconds).map_err(|_| {
            TgError::Other(format!(
                "send: files[{i}].duration is {seconds}; that is more seconds than Telegram takes ({})",
                i32::MAX
            ))
        })?,
    };

    let Some(waveform) = &file.waveform else {
        return Err(TgError::Other(format!(
            "send: files[{i}].waveform is missing; kind `voice` needs {WAVEFORM_LEVELS} levels, each 0-{WAVEFORM_MAX}"
        )));
    };
    if waveform.len() != WAVEFORM_LEVELS {
        return Err(TgError::Other(format!(
            "send: files[{i}].waveform has {} levels; kind `voice` needs exactly {WAVEFORM_LEVELS}, each 0-{WAVEFORM_MAX}",
            waveform.len()
        )));
    }
    let levels = waveform
        .iter()
        .enumerate()
        .map(|(j, &level)| {
            u8::try_from(level)
                .ok()
                .filter(|_| (0..=WAVEFORM_MAX).contains(&level))
                .ok_or_else(|| {
                    TgError::Other(format!(
                        "send: files[{i}].waveform[{j}] is {level}; each level is 0-{WAVEFORM_MAX}"
                    ))
                })
        })
        .collect::<Result<Vec<u8>>>()?;

    Ok(VoiceNote { duration, levels })
}

/// Require an absolute path that exists as a regular file and opens for
/// reading. TDLib uploads the file asynchronously *after* the send call
/// returns, so a path it cannot read surfaces as a failed send minutes later
/// on a message the caller was already told about — checking here is what
/// turns that into an in-band refusal with nothing sent.
fn validate_path(i: usize, raw: &str) -> Result<PathBuf> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(TgError::Other(format!(
            "send: files[{i}].path '{raw}' is not absolute; the daemon resolves no relative paths (its working directory is not the caller's)"
        )));
    }

    let meta = std::fs::metadata(path)
        .map_err(|e| TgError::Other(format!("send: files[{i}].path '{raw}' is unreadable: {e}")))?;
    if !meta.is_file() {
        return Err(TgError::Other(format!(
            "send: files[{i}].path '{raw}' is not a regular file"
        )));
    }
    // `metadata` follows the link and says nothing about permissions; opening
    // is the only check that answers "can TDLib read these bytes".
    std::fs::File::open(path)
        .map_err(|e| TgError::Other(format!("send: files[{i}].path '{raw}' is unreadable: {e}")))?;

    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn file_at(dir: &TempDir, name: &str) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, b"bytes").unwrap();
        path.to_string_lossy().into_owned()
    }

    fn entry(path: &str, kind: Option<&str>) -> SendFile {
        SendFile {
            path: path.to_string(),
            kind: kind.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn absent_files_is_a_text_send() {
        assert!(validate_files(None).unwrap().is_empty());
    }

    #[test]
    fn empty_files_array_is_refused() {
        let err = validate_files(Some(&[])).unwrap_err().to_string();
        assert!(err.contains("present but empty"), "{err}");
    }

    #[test]
    fn absent_kind_means_file() {
        let dir = TempDir::new().unwrap();
        let files = validate_files(Some(&[entry(&file_at(&dir, "a.pdf"), None)])).unwrap();
        assert_eq!(files[0].presentation, Presentation::File);
    }

    #[test]
    fn unknown_kind_is_refused_with_its_index() {
        let dir = TempDir::new().unwrap();
        let path = file_at(&dir, "a.jpg");
        let err = validate_files(Some(&[
            entry(&path, Some("photo")),
            entry(&path, Some("video")),
        ]))
        .unwrap_err()
        .to_string();
        assert!(err.contains("files[1]"), "{err}");
        assert!(err.contains("kind 'video'"), "{err}");
    }

    #[test]
    fn mixed_kinds_are_refused_before_any_path_is_touched() {
        // The paths are deliberately nonexistent: a mixed request has no
        // single-message rendering whatever is on disk, and the refusal must
        // name the split rather than the first missing file.
        let err = validate_files(Some(&[
            entry("/nope/a.jpg", Some("photo")),
            entry("/nope/b.pdf", Some("file")),
        ]))
        .unwrap_err()
        .to_string();
        assert!(err.contains("mixes kind `photo`"), "{err}");
        assert!(err.contains("files[1]"), "{err}");
        assert!(!err.contains("unreadable"), "{err}");
    }

    #[test]
    fn eleven_files_are_refused() {
        let entries: Vec<_> = (0..11).map(|_| entry("/nope/a.pdf", None)).collect();
        let err = validate_files(Some(&entries)).unwrap_err().to_string();
        assert!(err.contains("11 files requested"), "{err}");
        assert!(err.contains("at most 10"), "{err}");
    }

    #[test]
    fn ten_files_are_accepted() {
        let dir = TempDir::new().unwrap();
        let path = file_at(&dir, "a.pdf");
        let entries: Vec<_> = (0..10).map(|_| entry(&path, None)).collect();
        assert_eq!(validate_files(Some(&entries)).unwrap().len(), 10);
    }

    #[test]
    fn relative_path_is_refused() {
        let err = validate_files(Some(&[entry("a.pdf", None)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not absolute"), "{err}");
    }

    #[test]
    fn missing_file_is_refused() {
        let err = validate_files(Some(&[entry("/nonexistent/a.pdf", None)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unreadable"), "{err}");
    }

    #[test]
    fn directory_is_refused() {
        let dir = TempDir::new().unwrap();
        let err = validate_files(Some(&[entry(&dir.path().to_string_lossy(), None)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn unreadable_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("locked.pdf");
        std::fs::write(&path, b"bytes").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let refused = validate_files(Some(&[entry(&path.to_string_lossy(), None)]));
        // Running as root makes mode 0000 readable anyway; assert the refusal
        // only where the mode actually denies us.
        if std::fs::File::open(&path).is_ok() {
            return;
        }
        assert!(refused.unwrap_err().to_string().contains("unreadable"));
    }

    #[test]
    fn negative_dimension_is_refused() {
        let err = validate_files(Some(&[SendFile {
            path: "/nope/a.jpg".to_string(),
            kind: Some("photo".to_string()),
            width: Some(-1),
            ..Default::default()
        }]))
        .unwrap_err()
        .to_string();
        assert!(err.contains("cannot be negative"), "{err}");
    }

    #[test]
    fn dimension_on_a_document_is_refused() {
        let err = validate_files(Some(&[SendFile {
            path: "/nope/a.pdf".to_string(),
            kind: None,
            width: Some(640),
            ..Default::default()
        }]))
        .unwrap_err()
        .to_string();
        assert!(err.contains("read only for kind `photo`"), "{err}");
    }

    #[test]
    fn photo_dimensions_are_carried_through() {
        let dir = TempDir::new().unwrap();
        let files = validate_files(Some(&[SendFile {
            path: file_at(&dir, "a.jpg"),
            kind: Some("photo".to_string()),
            width: Some(800),
            height: Some(600),
            ..Default::default()
        }]))
        .unwrap();
        assert_eq!(
            files[0].presentation,
            Presentation::Photo {
                width: 800,
                height: 600
            }
        );
    }

    #[test]
    fn send_file_rejects_unknown_field() {
        let err = serde_json::from_value::<SendFile>(
            serde_json::json!({"path": "/a.jpg", "mime": "image/jpeg"}),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field `mime`"), "{err}");
        for field in ["path", "kind", "width", "height", "duration", "waveform"] {
            assert!(err.contains(field), "error should name `{field}`: {err}");
        }
    }

    fn voice(path: &str) -> SendFile {
        SendFile {
            path: path.to_string(),
            kind: Some("voice".to_string()),
            duration: Some(42),
            waveform: Some(vec![50; WAVEFORM_LEVELS]),
            ..Default::default()
        }
    }

    fn refusal(file: SendFile) -> String {
        validate_files(Some(&[file])).unwrap_err().to_string()
    }

    #[test]
    fn a_voice_note_carries_its_length_and_levels() {
        let dir = TempDir::new().unwrap();
        let mut file = voice(&file_at(&dir, "voice-note.ogg"));
        let levels: Vec<i64> = (0..64).map(|i| i * 100 / 63).collect();
        file.waveform = Some(levels.clone());
        let files = validate_files(Some(&[file])).unwrap();
        assert_eq!(
            files[0].presentation,
            Presentation::Voice(VoiceNote {
                duration: 42,
                levels: levels.iter().map(|&l| l as u8).collect(),
            })
        );
        assert_eq!(files[0].kind(), MediaKind::Voice);
    }

    #[test]
    fn a_voice_note_reads_from_the_wire_with_its_measures() {
        let file: SendFile = serde_json::from_value(serde_json::json!({
            "path": "/outbox/voice-note.ogg",
            "kind": "voice",
            "duration": 7,
            "waveform": vec![100; 64],
        }))
        .unwrap();
        assert_eq!(file.duration, Some(7));
        assert_eq!(file.waveform.as_deref(), Some(&[100; 64][..]));
    }

    #[test]
    fn a_voice_note_without_its_length_is_refused() {
        let mut file = voice("/nope/v.ogg");
        file.duration = None;
        let err = refusal(file);
        assert!(err.contains("files[0].duration is missing"), "{err}");
    }

    #[test]
    fn a_voice_note_shorter_than_a_second_is_refused() {
        for seconds in [0, -3] {
            let mut file = voice("/nope/v.ogg");
            file.duration = Some(seconds);
            let err = refusal(file);
            assert!(
                err.contains(&format!(
                    "duration is {seconds}; a voice note lasts 1 second or more"
                )),
                "{err}"
            );
        }
    }

    #[test]
    fn a_length_past_tdlibs_range_is_refused() {
        let mut file = voice("/nope/v.ogg");
        file.duration = Some(i64::from(i32::MAX) + 1);
        let err = refusal(file);
        assert!(err.contains("more seconds than Telegram takes"), "{err}");
    }

    #[test]
    fn a_voice_note_without_its_waveform_is_refused() {
        let mut file = voice("/nope/v.ogg");
        file.waveform = None;
        let err = refusal(file);
        assert!(err.contains("files[0].waveform is missing"), "{err}");
    }

    #[test]
    fn a_waveform_of_other_than_64_levels_is_refused() {
        for count in [0, 63, 65, 100] {
            let mut file = voice("/nope/v.ogg");
            file.waveform = Some(vec![0; count]);
            let err = refusal(file);
            assert!(
                err.contains(&format!(
                    "waveform has {count} levels; kind `voice` needs exactly 64"
                )),
                "{err}"
            );
        }
    }

    #[test]
    fn a_level_outside_0_to_100_is_refused_by_its_index() {
        for level in [-1, 101, 256, 1000] {
            let mut file = voice("/nope/v.ogg");
            let mut levels = vec![0; 64];
            levels[17] = level;
            file.waveform = Some(levels);
            let err = refusal(file);
            assert!(
                err.contains(&format!("waveform[17] is {level}; each level is 0-100")),
                "{err}"
            );
        }
    }

    #[test]
    fn a_dimension_on_a_voice_note_is_refused_even_a_zero() {
        for (width, height) in [(Some(0), None), (None, Some(480))] {
            let mut file = voice("/nope/v.ogg");
            file.width = width;
            file.height = height;
            let err = refusal(file);
            assert!(err.contains("is set but kind is `voice`"), "{err}");
            assert!(err.contains("read only for kind `photo`"), "{err}");
        }
    }

    #[test]
    fn a_length_or_waveform_on_another_kind_is_refused() {
        for kind in [None, Some("file"), Some("photo")] {
            let mut file = entry("/nope/a.jpg", kind);
            file.duration = Some(3);
            let err = refusal(file);
            assert!(err.contains("files[0].duration is set"), "{err}");
            assert!(err.contains("read only for kind `voice`"), "{err}");

            let mut file = entry("/nope/a.jpg", kind);
            file.waveform = Some(vec![0; 64]);
            let err = refusal(file);
            assert!(err.contains("files[0].waveform is set"), "{err}");
        }
    }

    #[test]
    fn a_voice_note_beside_any_other_file_is_refused_before_any_path_is_touched() {
        // Deliberately nonexistent paths: whatever is on disk, a voice note in
        // a several-file request has no single-message rendering.
        let cases = [
            (
                vec![voice("/nope/v.ogg"), entry("/nope/a.jpg", Some("photo"))],
                0,
            ),
            (vec![entry("/nope/a.pdf", None), voice("/nope/v.ogg")], 1),
            (vec![voice("/nope/v.ogg"), voice("/nope/w.ogg")], 0),
        ];
        for (files, at) in cases {
            let err = validate_files(Some(&files)).unwrap_err().to_string();
            assert!(
                err.contains(&format!(
                    "files[{at}] is kind `voice` and the request has 2 files"
                )),
                "{err}"
            );
            assert!(err.contains("a voice note is sent alone"), "{err}");
            assert!(!err.contains("unreadable"), "{err}");
        }
    }

    #[test]
    fn a_voice_note_alone_still_needs_a_readable_path() {
        let err = refusal(voice("/nonexistent/v.ogg"));
        assert!(err.contains("unreadable"), "{err}");
    }

    #[test]
    fn levels_scale_to_telegrams_five_bits_rounded() {
        let cases = [
            (0, 0),
            (1, 0),
            (2, 1),
            (4, 1),
            (5, 2),
            (50, 16),
            (98, 30),
            (99, 31),
            (100, 31),
        ];
        for (level, expected) in cases {
            assert_eq!(five_bit(level), expected, "level {level}");
        }
    }

    /// Telegram's reader of a 5-bit waveform, as tdesktop's
    /// `documentWaveformDecode` reads it: value `i` is the 5 bits at bit
    /// `5 * i` of the little-endian stream. An independent second form of the
    /// layout the packer writes.
    fn unpack(packed: &[u8]) -> Vec<u8> {
        (0..packed.len() * 8 / 5)
            .map(|i| {
                let (byte, shift) = (i * 5 / 8, i * 5 % 8);
                let low = u16::from(packed[byte]);
                let high = u16::from(packed.get(byte + 1).copied().unwrap_or(0));
                (((low | (high << 8)) >> shift) & 0x1F) as u8
            })
            .collect()
    }

    #[test]
    fn silence_packs_to_forty_zero_bytes() {
        assert_eq!(pack_waveform(&[0; 64]), vec![0u8; 40]);
        let note = VoiceNote {
            duration: 1,
            levels: vec![0; 64],
        };
        assert_eq!(note.tdlib_waveform(), "A".repeat(52) + "AA==");
    }

    #[test]
    fn full_loudness_packs_to_forty_set_bytes() {
        assert_eq!(pack_waveform(&[100; 64]), vec![0xFFu8; 40]);
        let note = VoiceNote {
            duration: 1,
            levels: vec![100; 64],
        };
        assert_eq!(note.tdlib_waveform(), "/".repeat(52) + "/w==");
    }

    #[test]
    fn the_first_level_takes_the_lowest_bits() {
        let one = |at: usize, level: u8| {
            let mut levels = [0u8; 64];
            levels[at] = level;
            pack_waveform(&levels)
        };
        // Bits 0-4, then bits 5-9 across the first byte boundary.
        assert_eq!(one(0, 100)[..2], [0x1F, 0x00]);
        assert_eq!(one(1, 100)[..2], [0xE0, 0x03]);
        // Scaled 1, 1, 1 (levels 3, 4, 2) at bits 0, 5 and 10.
        assert_eq!(pack_waveform(&[3, 4, 2])[..2], [0x21, 0x04]);
    }

    #[test]
    fn a_ramp_packs_as_telegrams_clients_pack_it() {
        // 5-bit values 0..=31 twice, reached from levels 0..=100: each value's
        // smallest level that rounds to it.
        let levels: Vec<u8> = (0..64)
            .map(|i| {
                (0..=100u8)
                    .find(|&l| five_bit(l) == (i % 32) as u8)
                    .unwrap()
            })
            .collect();
        let packed = pack_waveform(&levels);
        // Computed by the tdesktop encoder (uint16 OR at byte `5i/8`, shift
        // `5i%8`), independently of this packer.
        let twenty = [
            0x20, 0x88, 0x41, 0x8a, 0x39, 0x28, 0xa9, 0xc5, 0x9a, 0x7b, 0x30, 0xca, 0x49, 0xab,
            0xbd, 0x38, 0xeb, 0xcd, 0xbb, 0xff,
        ];
        assert_eq!(packed.len(), 40);
        assert_eq!(packed[..20], twenty);
        assert_eq!(packed[20..], twenty);
        assert_eq!(
            unpack(&packed),
            (0..64).map(|i| (i % 32) as u8).collect::<Vec<_>>()
        );
        let note = VoiceNote {
            duration: 2,
            levels,
        };
        assert_eq!(
            note.tdlib_waveform(),
            "IIhBijkoqcWaezDKSau9OOvNu/8giEGKOSipxZp7MMpJq7046827/w=="
        );
    }

    #[test]
    fn a_level_ramp_reads_back_scaled() {
        let levels: Vec<u8> = (0..64).map(|i| ((i * 100 + 31) / 63) as u8).collect();
        let packed = pack_waveform(&levels);
        assert_eq!(
            unpack(&packed),
            levels.iter().map(|&l| five_bit(l)).collect::<Vec<_>>()
        );
        let note = VoiceNote {
            duration: 2,
            levels,
        };
        assert_eq!(
            note.tdlib_waveform(),
            "IAQhxBiElGLOOQilpNRajLXm3HsQxijlnJTWau29GOOs9d6c9969/w=="
        );
    }

    #[test]
    fn display_name_is_the_basename() {
        let file = MediaFile {
            path: PathBuf::from("/outbox/item/0/Q3 report.pdf"),
            presentation: Presentation::File,
        };
        assert_eq!(file.display_name(), "Q3 report.pdf");
    }
}
