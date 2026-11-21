//! The editor toolbar's "Save to Zip" (a GameTable addition): packs the game folder into
//! `<folder name>.gamedef` in the game library (see [`crate::data::game_package`]). Asks before
//! replacing an earlier package, shows progress with Cancel, then says where the file went.

use std::path::Path;

use crate::app::Frame;
use crate::data::file_system::base_name;
use crate::data::game_package::{GamePackageJob, PackageError, game_package_path};
use crate::ui::draw::TextOptions;
use crate::ui::overlays::{ConfirmResponse, DialogAction, DialogResponse, DialogSpec};

enum Stage {
    ConfirmingReplace { game_folder_path: String, package_path: String },
    Packing(GamePackageJob),
    Finished(String),
}

pub struct SaveToZipFlow {
    stage: Stage,
}

/// "1.4 MB", or "850 KB" below a megabyte.
fn size_text(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn start_packing(game_folder_path: &str) -> Stage {
    match GamePackageJob::start(game_folder_path) {
        Ok(job) => Stage::Packing(job),
        Err(error) => Stage::Finished(format!("Could not save to zip: {error}")),
    }
}

impl SaveToZipFlow {
    /// Starts packing `game_folder_path`, or first asks whether to replace an existing package.
    pub fn open(game_folder_path: &str) -> SaveToZipFlow {
        let package_path = game_package_path(game_folder_path);
        let stage = if Path::new(&package_path).exists() {
            Stage::ConfirmingReplace { game_folder_path: game_folder_path.to_string(), package_path }
        } else {
            start_packing(game_folder_path)
        };
        SaveToZipFlow { stage }
    }

    /// Draws the current step; false once the flow is over.
    pub fn show(&mut self, frame: &mut Frame) -> bool {
        match &mut self.stage {
            Stage::ConfirmingReplace { game_folder_path, package_path } => {
                let message = format!("{} already exists. Replace it?", base_name(package_path));
                match frame.confirm_dialog("savetozipreplace", "Save to Zip", &message, "Replace", false) {
                    ConfirmResponse::Open => {}
                    ConfirmResponse::Cancelled => return false,
                    ConfirmResponse::Confirmed => self.stage = start_packing(game_folder_path),
                }
                true
            }
            Stage::Packing(job) => {
                if let Some(result) = job.poll() {
                    let file_name = base_name(&job.package_path).to_string();
                    match result {
                        Ok(summary) => {
                            let plural = if summary.file_count == 1 { "" } else { "s" };
                            self.stage = Stage::Finished(format!(
                                "Saved {file_name} ({} file{plural}, {}) to:\n{}",
                                summary.file_count,
                                size_text(summary.package_bytes),
                                job.package_path
                            ));
                        }
                        Err(PackageError::Cancelled) => {
                            frame.ui.toast(&format!("Save to Zip was cancelled; {file_name} was not written."), None);
                            return false;
                        }
                        Err(PackageError::Failed(error)) => {
                            self.stage = Stage::Finished(format!("Could not save {file_name}: {error}"));
                        }
                    }
                    return true;
                }
                Self::show_progress(frame, job);
                true
            }
            Stage::Finished(message) => !frame.alert_dialog("savetozipdone", "Save to Zip", message),
        }
    }

    fn show_progress(frame: &mut Frame, job: &GamePackageJob) {
        let (done, total) = (job.files_done(), job.total_files);
        let cancelling = job.is_cancelling();
        let status = if cancelling { "Stopping...".to_string() } else { format!("{done} of {total} files") };
        let title = format!("Saving {}", base_name(&job.package_path));
        let actions = [DialogAction::text("Cancel").disabled(cancelling)];
        let spec = DialogSpec {
            title: Some(&title),
            width: 420.0,
            actions: &actions,
            dismissible: false,
            ..DialogSpec::default()
        };
        let progress = if total == 0 { None } else { Some(done as f32 / total as f32) };
        let response = frame.dialog("savetozipprogress", &spec, |frame, x, y, width| {
            frame.text(&status, x, y, TextOptions::new().width(width));
            frame.progress_bar(x, y + 28.0, width, 4.0, progress, None);
            40.0
        });
        if response == DialogResponse::Action(0) {
            job.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_kilobytes_then_megabytes() {
        assert_eq!(size_text(1), "1 KB");
        assert_eq!(size_text(850 * 1024), "850 KB");
        assert_eq!(size_text(1536 * 1024), "1.5 MB");
    }
}
