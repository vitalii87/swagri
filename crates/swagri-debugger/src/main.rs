#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use sysinfo::System;

const MAX_LOG_LINES: usize = 2_000;
const MAX_METRIC_SAMPLES: usize = 240;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1120.0, 760.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Swagri Debugger",
        options,
        Box::new(|creation_context| Ok(Box::new(DebuggerApp::new(creation_context)))),
    )
}

struct ManagedAgent {
    child: Child,
    stdin: ChildStdin,
}

impl ManagedAgent {
    fn send(&mut self, command: &str) -> Result<()> {
        writeln!(self.stdin, "{command}").context("could not write to the agent console")?;
        self.stdin.flush().context("could not flush agent input")
    }
}

impl Drop for ManagedAgent {
    fn drop(&mut self) {
        let _ = self.send("quit");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct DebuggerApp {
    agent: Option<ManagedAgent>,
    agent_path: PathBuf,
    identity_path: PathBuf,
    node_name: String,
    listen_address: String,
    command: String,
    logs: VecDeque<String>,
    output_tx: Sender<String>,
    output_rx: Receiver<String>,
    peer_id: Option<String>,
    listen_addresses: Vec<String>,
    completed_tasks: u64,
    system: System,
    last_refresh: Instant,
    sample_index: f64,
    cpu_history: VecDeque<[f64; 2]>,
    memory_history: VecDeque<[f64; 2]>,
}

impl DebuggerApp {
    fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        creation_context.egui_ctx.set_visuals(egui::Visuals::dark());
        let (output_tx, output_rx) = mpsc::channel();
        let identity_path = default_identity_path();

        Self {
            agent: None,
            agent_path: sibling_agent_path(),
            identity_path,
            node_name: "swagri-debugger".into(),
            listen_address: "/ip4/0.0.0.0/udp/0/quic-v1".into(),
            command: String::new(),
            logs: VecDeque::from(["Debugger ready. Start the local agent to begin.".into()]),
            output_tx,
            output_rx,
            peer_id: None,
            listen_addresses: Vec::new(),
            completed_tasks: 0,
            system: System::new_all(),
            last_refresh: Instant::now() - Duration::from_secs(2),
            sample_index: 0.0,
            cpu_history: VecDeque::new(),
            memory_history: VecDeque::new(),
        }
    }

    fn start_agent(&mut self) {
        if self.agent.is_some() {
            return;
        }

        match spawn_agent(
            &self.agent_path,
            &self.node_name,
            &self.identity_path,
            &self.listen_address,
            &self.output_tx,
        ) {
            Ok(agent) => {
                self.push_log(format!("Started {}", self.agent_path.display()));
                self.agent = Some(agent);
            }
            Err(error) => self.push_log(format!("ERROR: {error:#}")),
        }
    }

    fn stop_agent(&mut self) {
        if let Some(mut agent) = self.agent.take() {
            let _ = agent.send("quit");
            self.push_log("Stop requested.".into());
        }
    }

    fn send_command(&mut self) {
        let command = self.command.trim().to_owned();
        if command.is_empty() {
            return;
        }

        self.push_log(format!("> {command}"));
        if let Some(agent) = self.agent.as_mut() {
            if let Err(error) = agent.send(&command) {
                self.push_log(format!("ERROR: {error:#}"));
            }
        } else {
            self.push_log("ERROR: Start the agent first.".into());
        }
        self.command.clear();
    }

    fn poll_agent(&mut self) {
        while let Ok(line) = self.output_rx.try_recv() {
            if let Some(value) = line.strip_prefix("Peer ID: ") {
                self.peer_id = Some(value.to_owned());
            }
            if let Some(value) = line.strip_prefix("Listening on ")
                && !self.listen_addresses.iter().any(|item| item == value)
            {
                self.listen_addresses.push(value.to_owned());
            }
            if line.starts_with("Result from ") {
                self.completed_tasks += 1;
            }
            self.push_log(line);
        }

        let exit = self
            .agent
            .as_mut()
            .and_then(|agent| agent.child.try_wait().ok().flatten());
        if let Some(status) = exit {
            self.agent = None;
            self.push_log(format!("Agent exited with {status}."));
        }
    }

    fn refresh_metrics(&mut self) {
        if self.last_refresh.elapsed() < Duration::from_secs(1) {
            return;
        }

        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.sample_index += 1.0;
        let cpu = f64::from(self.system.global_cpu_usage());
        let total = self.system.total_memory().max(1);
        let memory = self.system.used_memory() as f64 / total as f64 * 100.0;
        push_sample(&mut self.cpu_history, [self.sample_index, cpu]);
        push_sample(&mut self.memory_history, [self.sample_index, memory]);
        self.last_refresh = Instant::now();
    }

    fn push_log(&mut self, line: String) {
        if self.logs.len() == MAX_LOG_LINES {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    fn draw_status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (status, color) = if self.agent.is_some() {
                ("RUNNING", Color32::from_rgb(70, 210, 130))
            } else {
                ("STOPPED", Color32::from_rgb(230, 100, 90))
            };
            ui.heading("Swagri Debugger");
            ui.label(RichText::new(status).color(color).strong());
            ui.separator();
            ui.label(format!("Completed remote tasks: {}", self.completed_tasks));
        });
    }

    fn draw_configuration(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("configuration")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Agent binary");
                path_editor(ui, &mut self.agent_path);
                ui.end_row();
                ui.label("Node name");
                ui.text_edit_singleline(&mut self.node_name);
                ui.end_row();
                ui.label("Identity");
                path_editor(ui, &mut self.identity_path);
                ui.end_row();
                ui.label("Listen address");
                ui.text_edit_singleline(&mut self.listen_address);
                ui.end_row();
            });

        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.agent.is_none(), egui::Button::new("Start agent"))
                .clicked()
            {
                self.start_agent();
            }
            if ui
                .add_enabled(self.agent.is_some(), egui::Button::new("Stop agent"))
                .clicked()
            {
                self.stop_agent();
            }
            if ui.button("Clear log").clicked() {
                self.logs.clear();
            }
        });

        ui.label(format!(
            "Peer ID: {}",
            self.peer_id.as_deref().unwrap_or("not available")
        ));
        if !self.listen_addresses.is_empty() {
            ui.label(format!("Listening: {}", self.listen_addresses.join(", ")));
        }
    }

    fn draw_metrics(&self, ui: &mut egui::Ui) {
        let used_gib = self.system.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let total_gib = self.system.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let cpu = self.cpu_history.back().map_or(0.0, |sample| sample[1]);

        ui.label(format!(
            "Host CPU {cpu:.1}%    Memory {used_gib:.2}/{total_gib:.2} GiB"
        ));
        Plot::new("host_metrics")
            .height(170.0)
            .include_y(0.0)
            .include_y(100.0)
            .allow_drag(false)
            .allow_zoom(false)
            .show(ui, |plot_ui| {
                plot_ui.line(
                    Line::new(
                        "CPU %",
                        PlotPoints::from_iter(self.cpu_history.iter().copied()),
                    )
                    .color(Color32::from_rgb(90, 190, 255)),
                );
                plot_ui.line(
                    Line::new(
                        "Memory %",
                        PlotPoints::from_iter(self.memory_history.iter().copied()),
                    )
                    .color(Color32::from_rgb(255, 180, 80)),
                );
            });
    }

    fn draw_console(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Agent console").strong());
        egui::ScrollArea::vertical()
            .id_salt("agent_log")
            .stick_to_bottom(true)
            .max_height(230.0)
            .show(ui, |ui| {
                for line in &self.logs {
                    ui.monospace(line);
                }
            });

        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.command)
                    .hint_text("help, peers, echo <peer-id> <text> ...")
                    .desired_width(f32::INFINITY),
            );
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui.button("Send").clicked() || enter {
                self.send_command();
            }
        });
    }
}

impl eframe::App for DebuggerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_agent();
        self.refresh_metrics();

        egui::CentralPanel::default().show(ui, |ui| {
            self.draw_status(ui);
            ui.separator();
            ui.collapsing("Agent configuration", |ui| self.draw_configuration(ui));
            ui.separator();
            self.draw_metrics(ui);
            ui.separator();
            self.draw_console(ui);
        });

        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
}

fn spawn_agent(
    path: &PathBuf,
    node_name: &str,
    identity_path: &PathBuf,
    listen_address: &str,
    output_tx: &Sender<String>,
) -> Result<ManagedAgent> {
    if !path.is_file() {
        bail!("agent binary was not found at {}", path.display());
    }

    let mut command = Command::new(path);
    command
        .args(["--name", node_name, "--identity"])
        .arg(identity_path)
        .args(["--listen", listen_address])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_child_process(&mut command);

    let mut child = command
        .spawn()
        .context("could not start the Swagri agent")?;
    let stdin = child.stdin.take().context("agent stdin was unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("agent stdout was unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("agent stderr was unavailable")?;
    pump_output(stdout, output_tx.clone());
    pump_output(stderr, output_tx.clone());

    Ok(ManagedAgent { child, stdin })
}

fn pump_output(reader: impl std::io::Read + Send + 'static, output_tx: Sender<String>) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => {
                    let _ = output_tx.send(line);
                }
                Err(error) => {
                    let _ = output_tx.send(format!("ERROR reading agent output: {error}"));
                    break;
                }
            }
        }
    });
}

#[cfg(windows)]
fn configure_child_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_child_process(_command: &mut Command) {}

fn sibling_agent_path() -> PathBuf {
    let executable_name = if cfg!(windows) {
        "swagri-agent.exe"
    } else {
        "swagri-agent"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(executable_name)))
        .unwrap_or_else(|| PathBuf::from(executable_name))
}

fn default_identity_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Swagri")
        .join("debugger.key")
}

fn path_editor(ui: &mut egui::Ui, path: &mut PathBuf) {
    let mut text = path.to_string_lossy().into_owned();
    if ui.text_edit_singleline(&mut text).changed() {
        *path = PathBuf::from(text);
    }
}

fn push_sample(history: &mut VecDeque<[f64; 2]>, sample: [f64; 2]) {
    if history.len() == MAX_METRIC_SAMPLES {
        history.pop_front();
    }
    history.push_back(sample);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_history_is_bounded() {
        let mut history = VecDeque::new();
        for index in 0..(MAX_METRIC_SAMPLES + 10) {
            push_sample(&mut history, [index as f64, 1.0]);
        }
        assert_eq!(history.len(), MAX_METRIC_SAMPLES);
        assert_eq!(history.front(), Some(&[10.0, 1.0]));
    }

    #[test]
    fn sibling_binary_uses_agent_name() {
        let filename = sibling_agent_path()
            .file_name()
            .expect("agent filename")
            .to_string_lossy()
            .to_string();
        assert!(filename.starts_with("swagri-agent"));
    }
}
