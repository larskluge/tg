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
//! silently split into two messages.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, TgError};

/// TDLib albums carry 2-10 contents; a single file is sent as one message.
pub const MAX_FILES: usize = 10;

/// Wire mirror of one `args.files[]` entry. `deny_unknown_fields` for the same
/// reason [`crate::commands::send::SendRequest`] carries it: a dropped key here
/// means a file delivered with the wrong presentation and `ok:true`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SendFile {
    /// Absolute path to the file. Its basename is what the recipient sees.
    pub path: String,
    /// `photo` or `file`; absent means `file`. Kept as a string so this struct
    /// stays a transparent wire mirror and the error text stays ours.
    #[serde(default)]
    pub kind: Option<String>,
    /// Photo pixel width; `0` or absent lets TDLib work it out.
    #[serde(default)]
    pub width: Option<i32>,
    /// Photo pixel height; `0` or absent lets TDLib work it out.
    #[serde(default)]
    pub height: Option<i32>,
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
}

impl MediaKind {
    /// Strict, case-sensitive, absent means `file`. An unrecognised value is a
    /// caller bug and is refused rather than downgraded, because the fallback
    /// would change how the recipient sees the message.
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value {
            None | Some("file") => Ok(Self::File),
            Some("photo") => Ok(Self::Photo),
            Some(other) => Err(TgError::Other(format!(
                "kind '{other}' is not supported. Expected `photo` or `file`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::File => "file",
        }
    }
}

/// A validated attachment: an existing, readable, absolute path plus the
/// presentation the caller asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFile {
    pub path: PathBuf,
    pub kind: MediaKind,
    /// `0` means "let TDLib work it out"; always `0` for [`MediaKind::File`].
    pub width: i32,
    pub height: i32,
}

impl MediaFile {
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

    // Shape first, filesystem second: the kind and dimension faults cost
    // nothing to find, and the mixed-kind refusal is about the request as a
    // whole, so it must be reported before any single path is blamed.
    let mut kinds = Vec::with_capacity(files.len());
    for (i, file) in files.iter().enumerate() {
        let kind = MediaKind::parse(file.kind.as_deref())
            .map_err(|e| TgError::Other(format!("send: files[{i}]: {e}")))?;
        validate_dimensions(i, file, kind)?;
        kinds.push(kind);
    }

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
        .zip(kinds)
        .enumerate()
        .map(|(i, (file, kind))| {
            let path = validate_path(i, &file.path)?;
            Ok(MediaFile {
                path,
                kind,
                width: dimension(file.width, kind),
                height: dimension(file.height, kind),
            })
        })
        .collect()
}

fn validate_dimensions(i: usize, file: &SendFile, kind: MediaKind) -> Result<()> {
    for (name, value) in [("width", file.width), ("height", file.height)] {
        let Some(value) = value else { continue };
        if value < 0 {
            return Err(TgError::Other(format!(
                "send: files[{i}].{name} is {value}; pixel dimensions cannot be negative (use 0 to let Telegram work it out)"
            )));
        }
        // Refusing rather than ignoring: a dimension on a document says
        // something about the payload that `kind: file` cannot honour, and a
        // dropped field here is how a caller ends up believing it asked for a
        // photo.
        if value != 0 && kind != MediaKind::Photo {
            return Err(TgError::Other(format!(
                "send: files[{i}].{name} is set but kind is `{}`; dimensions are read only for kind `photo`",
                kind.as_str()
            )));
        }
    }
    Ok(())
}

fn dimension(value: Option<i32>, kind: MediaKind) -> i32 {
    match kind {
        MediaKind::Photo => value.unwrap_or(0),
        MediaKind::File => 0,
    }
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
        assert_eq!(files[0].kind, MediaKind::File);
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
            height: None,
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
            height: None,
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
        }]))
        .unwrap();
        assert_eq!((files[0].width, files[0].height), (800, 600));
    }

    #[test]
    fn send_file_rejects_unknown_field() {
        let err = serde_json::from_value::<SendFile>(
            serde_json::json!({"path": "/a.jpg", "mime": "image/jpeg"}),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field `mime`"), "{err}");
        for field in ["path", "kind", "width", "height"] {
            assert!(err.contains(field), "error should name `{field}`: {err}");
        }
    }

    #[test]
    fn display_name_is_the_basename() {
        let file = MediaFile {
            path: PathBuf::from("/outbox/item/0/Q3 report.pdf"),
            kind: MediaKind::File,
            width: 0,
            height: 0,
        };
        assert_eq!(file.display_name(), "Q3 report.pdf");
    }
}
