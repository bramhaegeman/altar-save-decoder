#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod decoder;
mod interpreter;

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

enum Status {
    Idle,
    Working(String),
    Done { output_path: PathBuf, summary: String },
    Error(String),
}

struct App {
    status: Status,
    rx: Option<Receiver<Status>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            rx: None,
        }
    }
}

fn run_decode(path: PathBuf, tx: Sender<Status>) {
    let _ = tx.send(Status::Working("Reading save file...".into()));

    let altar_bytes = match decoder::extract_altar_bytes(&path) {
        Ok(b) => b,
        Err(e) => {
            let _ = tx.send(Status::Error(format!("Couldn't read this file: {e}")));
            return;
        }
    };

    let _ = tx.send(Status::Working(format!(
        "Decompressing {} KB of save data (this can take a little while for long-played characters)...",
        altar_bytes.len() / 1024
    )));

    let tx_progress = tx.clone();
    let mut last_report = std::time::Instant::now();
    let mut progress = move |consumed: usize, total: usize| {
        if last_report.elapsed().as_millis() > 200 {
            last_report = std::time::Instant::now();
            let pct = (consumed as f64 / total.max(1) as f64 * 100.0) as u32;
            let _ = tx_progress.send(Status::Working(format!("Decompressing... {pct}%")));
        }
    };

    let (decoded, report) = decoder::decode_chunk_chain(&altar_bytes, &mut progress);

    if report.chunks_decoded == 0 {
        let _ = tx.send(Status::Error(
            "No Oodle-compressed chunks were found in this file. It may not be an Oblivion Remastered save, or the format has changed.".into(),
        ));
        return;
    }

    let _ = tx.send(Status::Working("Interpreting property list...".into()));
    let interpreted = interpreter::interpret(&decoded);

    let _ = tx.send(Status::Working("Writing text output...".into()));

    let mut header = String::new();
    header.push_str("================================================================\n");
    header.push_str(" Oblivion Remastered Save Decoder — decoded output\n");
    header.push_str(" Tool by Bram Haegeman\n");
    header.push_str("================================================================\n\n");
    header.push_str(&format!("Source file:              {}\n", path.display()));
    header.push_str(&format!("Source size (compressed): {} bytes\n", report.source_bytes));
    header.push_str(&format!("Chunks decoded:           {}\n", report.chunks_decoded));
    header.push_str(&format!("Decoded size:             {} bytes\n", report.decoded_bytes));
    if report.tail_bytes_left_raw > 0 {
        header.push_str(&format!(
            "Note: {} trailing bytes were not part of the Oodle chunk stream (likely the save's\n      preview metadata / thumbnail image block) and are not included below.\n",
            report.tail_bytes_left_raw
        ));
    }
    header.push_str("\n---------------------------------------------------------------\n\n");

    // Written next to the tool itself, never into the save folder — this app never creates
    // or modifies anything inside the user's SaveGames directory.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("save").to_string();

    let readable_header = format!(
        "{header}This is a best-effort readable listing of every top-level property found in\n\
         the save. Simple values (numbers, text, yes/no flags) are shown directly. Complex\n\
         ones (structs, arrays, maps — where quest, item and NPC data lives) are shown by\n\
         name, type and size, since decoding their internal layout is a separate effort.\n\
         See the README for details.\n\n\
         Top-level properties found: {}\n\
         {}\n\n\
         ---------------------------------------------------------------\n\n",
        interpreted.properties_found,
        if interpreted.stopped_early {
            "(the walk stopped before the end — see the note at the bottom of this file)"
        } else {
            "(reached the end of the readable property list cleanly)"
        }
    );

    let readable_path = exe_dir.join(format!("{stem}_readable.txt"));
    if let Err(e) = std::fs::write(&readable_path, format!("{readable_header}{}", interpreted.text)) {
        let _ = tx.send(Status::Error(format!("Decoded successfully but couldn't write the readable output file: {e}")));
        return;
    }

    let hex_dump = decoder::to_hex_dump(&decoded);
    let hexdump_path = exe_dir.join(format!("{stem}_hexdump.txt"));
    let _ = std::fs::write(&hexdump_path, format!("{header}{hex_dump}"));

    let summary = format!(
        "Decoded {} chunks ({} bytes). Found {} readable top-level properties.",
        report.chunks_decoded, report.decoded_bytes, interpreted.properties_found
    );
    let _ = tx.send(Status::Done { output_path: readable_path, summary });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.rx {
            if let Ok(status) = rx.try_recv() {
                self.status = status;
            }
        }
        if matches!(self.status, Status::Working(_)) {
            ctx.request_repaint();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.heading("Oblivion Remastered Save Decoder");
                ui.label(
                    egui::RichText::new("Unpacks the game's Oodle-compressed save data into a plain-text dump")
                        .weak(),
                );
            });

            ui.add_space(32.0);

            ui.vertical_centered(|ui| {
                let busy = matches!(self.status, Status::Working(_));
                let button = egui::Button::new(egui::RichText::new("  Choose a .sav file...  ").size(16.0))
                    .min_size(egui::vec2(240.0, 44.0));
                if ui.add_enabled(!busy, button).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Oblivion Remastered save", &["sav"])
                        .set_title("Choose an Oblivion Remastered save file")
                        .pick_file()
                    {
                        let (tx, rx) = channel();
                        self.rx = Some(rx);
                        self.status = Status::Working("Starting...".into());
                        std::thread::spawn(move || run_decode(path, tx));
                    }
                }
            });

            ui.add_space(28.0);

            ui.vertical_centered(|ui| match &self.status {
                Status::Idle => {
                    ui.label("Pick a save file to begin. Nothing is modified — this only reads a copy of the data.");
                }
                Status::Working(msg) => {
                    ui.spinner();
                    ui.add_space(8.0);
                    ui.label(msg);
                }
                Status::Done { output_path, summary } => {
                    ui.colored_label(egui::Color32::from_rgb(120, 200, 120), "Done!");
                    ui.add_space(6.0);
                    ui.label(summary);
                    ui.add_space(6.0);
                    ui.label(format!("Saved to: {}", output_path.display()));
                    ui.add_space(10.0);
                    if ui.button("Open containing folder").clicked() {
                        if let Some(dir) = output_path.parent() {
                            let _ = std::process::Command::new("explorer").arg(dir).spawn();
                        }
                    }
                }
                Status::Error(msg) => {
                    ui.colored_label(egui::Color32::from_rgb(220, 120, 120), "Something went wrong");
                    ui.add_space(6.0);
                    ui.label(msg);
                }
            });
        });

        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Oblivion Remastered Save Decoder — by Bram Haegeman").small().weak());
            });
            ui.add_space(4.0);
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 380.0])
            .with_min_inner_size([420.0, 320.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Oblivion Remastered Save Decoder",
        options,
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}
