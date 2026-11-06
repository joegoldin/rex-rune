//! Terminal-initiated copies (OSC 52 and server-side selection). Clipboard reads are
//! deliberately unsupported: a remote program must never read the desktop clipboard.
use super::{Client, HostModel, request};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const MAX_COPY_BYTES: usize = 8 * 1024 * 1024;

/// One pending copy, replaced by a newer copy rather than accumulating terminal output.
/// Do not derive Debug: clipboard contents may be sensitive.
#[derive(Clone)]
pub struct ClipboardCopy {
    pub session: String,
    pub block: String,
    pub text: String,
}

pub fn is_copy(payload: &Value) -> bool {
    payload["creator_name"] == super::activity::TERMINAL
        && matches!(
            payload["name"].as_str(),
            Some("clipboard_written" | "selection_copied")
        )
}

/// Only the terminal the user is interacting with may change the clipboard. The UI
/// checks window focus again when it consumes the result, after any asynchronous read.
pub fn accepts(model: &HostModel, session: &str, block: &str) -> bool {
    model.visible && model.selected.as_deref() == Some(session) && model.focused() == Some(block)
}

pub async fn read(control: &mut Client, session: &str, payload: &Value) -> Result<ClipboardCopy> {
    let block = payload["block_id"]
        .as_str()
        .context("Missing clipboard block")?;
    let body = &payload["payload"];
    let text = if let Some(text) = body["text"].as_str() {
        ensure!(
            text.len() <= MAX_COPY_BYTES,
            "Terminal copy exceeds the 8 MiB clipboard limit"
        );
        text.to_owned()
    } else {
        // Pin the result id on every request: never silently copy a newer result halfway
        // through this one. Offsets count UTF-8 bytes, not characters or JSON bytes.
        let id = body["id"]
            .as_u64()
            .filter(|id| *id != 0)
            .context("Missing clipboard copy id")?;
        if let Some(bytes) = body["bytes"].as_u64() {
            ensure!(
                bytes <= MAX_COPY_BYTES as u64,
                "Terminal copy exceeds the 8 MiB clipboard limit"
            );
        }
        let method = if payload["name"] == "clipboard_written" {
            "com.superlogical.terminal.clipboard_read"
        } else {
            "com.superlogical.terminal.blob_read"
        };
        let mut text = String::new();
        let mut total = None;
        loop {
            let value = request(
                control,
                method,
                json!({"session_id": session, "block_id": block,
                "args": {"id": id, "offset": text.len(), "max_bytes": 49152}}),
            )
            .await?;
            let part: rexrs::api::terminal::BlobReadResponse = serde_json::from_value(value)?;
            ensure!(part.id == id, "Clipboard copy changed while reading");
            ensure!(
                part.total >= 0 && part.total as u64 <= MAX_COPY_BYTES as u64,
                "Terminal copy exceeds the 8 MiB clipboard limit"
            );
            let size = part.total as usize;
            ensure!(
                total.is_none_or(|previous| previous == size),
                "Clipboard copy size changed while reading"
            );
            total = Some(size);
            ensure!(
                text.len() + part.data.len() <= size,
                "Clipboard copy exceeded its declared size"
            );
            ensure!(
                part.eof || !part.data.is_empty(),
                "Clipboard read made no progress"
            );
            text.push_str(&part.data);
            if part.eof {
                ensure!(
                    text.len() == size,
                    "Clipboard copy ended before all text arrived"
                );
                break text;
            }
        }
    };
    Ok(ClipboardCopy {
        session: session.into(),
        block: block.into(),
        text,
    })
}
