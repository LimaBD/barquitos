// ─── Rotating JSONL File Writer ──────────────────────────────────────────────
//
// Writes raw AIS JSON messages to JSONL files, rotating based on UTC minute
// and optional maximum message count per file.
//
// Before every write, verifies the mount point is healthy via a timed check.
// ────────────────────────────────────────────────────────────────────────────

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use chrono::Utc;
use log::info;

use crate::config;
use crate::mount_check;

pub struct RotatingFileWriter {
    output_dir: PathBuf,
    max_messages: u64,
    file: Option<BufWriter<File>>,
    current_minute: Option<String>,
    file_index: u32,
    message_count: u64,
    total_messages: u64,
}

impl RotatingFileWriter {
    pub fn new(output_dir: &str, max_messages: u64) -> Self {
        Self {
            output_dir: PathBuf::from(output_dir),
            max_messages,
            file: None,
            current_minute: None,
            file_index: 0,
            message_count: 0,
            total_messages: 0,
        }
    }

    fn minute_tag(&self) -> String {
        Utc::now().format("%Y%m%d_%H%M").to_string()
    }

    fn open_new_file(&mut self) -> Result<(), std::io::Error> {
        self.close();

        let minute = self.minute_tag();
        if self.current_minute.as_deref() != Some(&minute) {
            self.current_minute = Some(minute.clone());
            self.file_index = 0;
        }

        // Ensure output directory exists
        fs::create_dir_all(&self.output_dir)?;

        let mut filename = format!("ais_{}_{:04}.jsonl", minute, self.file_index);
        let mut filepath = self.output_dir.join(&filename);

        // Bump index if file already exists (e.g. restart in the same minute)
        while filepath.exists() {
            self.file_index += 1;
            filename = format!("ais_{}_{:04}.jsonl", minute, self.file_index);
            filepath = self.output_dir.join(&filename);
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&filepath)?;

        self.file = Some(BufWriter::with_capacity(64 * 1024, file));
        self.message_count = 0;
        info!("Writing to {}", filepath.display());
        Ok(())
    }

    /// Write a single raw JSON message (one line).
    /// Returns Ok(true) if written, Ok(false) if mount check failed.
    pub fn write(&mut self, raw_json: &str) -> Result<bool, std::io::Error> {
        // Check mount before writing
        if !mount_check::ensure_mount_healthy(
            config::MOUNT_POINT,
            config::MOUNT_CHECK_TIMEOUT_SECS,
        ) {
            return Ok(false);
        }

        let minute = self.minute_tag();
        let needs_rotation = self.file.is_none()
            || self.current_minute.as_deref() != Some(&minute)
            || (self.max_messages > 0 && self.message_count >= self.max_messages);

        if needs_rotation {
            self.open_new_file()?;
        }

        if let Some(ref mut writer) = self.file {
            let line = raw_json.trim_end_matches('\n');
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            self.message_count += 1;
            self.total_messages += 1;
        }

        Ok(true)
    }

    pub fn total_messages(&self) -> u64 {
        self.total_messages
    }

    fn close(&mut self) {
        if let Some(ref mut writer) = self.file {
            let _ = writer.flush();
        }
        self.file = None;
    }
}

impl Drop for RotatingFileWriter {
    fn drop(&mut self) {
        self.close();
    }
}
