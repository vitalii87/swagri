#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use semver::Version;
use sysinfo::System;

const MAX_LOG_LINES: usize = 2_000;
const MAX_METRIC_SAMPLES: usize = 240;
const DOWNLOADS_URL: &str = "https://github.com/vitalii87/swagri/actions/workflows/packages.yml";

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 820.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Swagri Debugger",
        options,
        Box::new(|context| Ok(Box::new(DebuggerApp::new(context)))),
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PeerState {
    #[default]
    Discovered,
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

impl PeerState {
    fn label(self) -> &'static str {
        match self {
            Self::Discovered => "знайдено",
            Self::Connecting => "підключення",
            Self::Connected => "підключено",
            Self::Disconnected => "відключено",
            Self::Failed => "помилка",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Self::Connected => Color32::from_rgb(70, 210, 130),
            Self::Connecting => Color32::from_rgb(255, 190, 70),
            Self::Failed => Color32::from_rgb(240, 90, 80),
            Self::Discovered | Self::Disconnected => Color32::LIGHT_GRAY,
        }
    }
}

#[derive(Default)]
struct PeerView {
    addresses: Vec<String>,
    state: PeerState,
    version: Option<String>,
    last_message: String,
    trusted_for_updates: bool,
    update_progress: Option<(u64, u64)>,
    resources: Option<ResourceView>,
}

#[derive(Clone, Debug)]
struct ResourceView {
    observed_at_unix_ms: u64,
    os: String,
    arch: String,
    cpu_brand: String,
    physical_cores: u16,
    logical_cores: u16,
    total_memory_bytes: u64,
    available_memory_bytes: u64,
    host_cpu_percent: f32,
    agent_cpu_percent: f32,
    agent_memory_bytes: u64,
    active_tasks: u32,
    cpu_limit_percent: f32,
    memory_limit_percent: f32,
    allocatable_memory_bytes: u64,
    calibrated_cpu_score: f64,
    effective_cpu_score: f64,
}

struct DebuggerApp {
    agent: Option<ManagedAgent>,
    agent_path: PathBuf,
    updater_path: PathBuf,
    identity_path: PathBuf,
    node_name: String,
    listen_address: String,
    dial_address: String,
    command: String,
    logs: VecDeque<String>,
    notices: VecDeque<String>,
    output_tx: Sender<String>,
    output_rx: Receiver<String>,
    local_peer_id: Option<String>,
    local_version: String,
    listen_addresses: Vec<String>,
    peers: BTreeMap<String, PeerView>,
    selected_peer: Option<String>,
    automatic_peer_updates: bool,
    requested_updates: BTreeSet<String>,
    ready_update: Option<(String, String, PathBuf)>,
    updater_child: Option<Child>,
    completed_tasks: u64,
    local_resources: Option<ResourceView>,
    max_cpu_percent: f32,
    max_memory_percent: f32,
    show_raw_console: bool,
    close_requested: bool,
    system: System,
    last_refresh: Instant,
    sample_index: f64,
    cpu_history: VecDeque<[f64; 2]>,
    memory_history: VecDeque<[f64; 2]>,
}

impl DebuggerApp {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        context.egui_ctx.set_visuals(egui::Visuals::dark());
        let (output_tx, output_rx) = mpsc::channel();

        Self {
            agent: None,
            agent_path: sibling_agent_path(),
            updater_path: sibling_updater_path(),
            identity_path: default_identity_path(),
            node_name: default_node_name(),
            listen_address: "/ip4/0.0.0.0/udp/0/quic-v1".into(),
            dial_address: String::new(),
            command: String::new(),
            logs: VecDeque::new(),
            notices: VecDeque::from([
                "1. Запустіть агент. 2. Натисніть «Знайти агентів». 3. Оберіть peer і перевірте зв'язок."
                    .into(),
            ]),
            output_tx,
            output_rx,
            local_peer_id: None,
            local_version: env!("CARGO_PKG_VERSION").into(),
            listen_addresses: Vec::new(),
            peers: BTreeMap::new(),
            selected_peer: None,
            automatic_peer_updates: false,
            requested_updates: BTreeSet::new(),
            ready_update: None,
            updater_child: None,
            completed_tasks: 0,
            local_resources: None,
            max_cpu_percent: 75.0,
            max_memory_percent: 50.0,
            show_raw_console: false,
            close_requested: false,
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
        self.local_peer_id = None;
        self.local_resources = None;
        self.listen_addresses.clear();
        self.peers.clear();
        self.selected_peer = None;

        match spawn_agent(
            &self.agent_path,
            &self.node_name,
            &self.identity_path,
            &self.listen_address,
            &self.updater_path,
            self.max_cpu_percent,
            self.max_memory_percent,
            &self.output_tx,
        ) {
            Ok(agent) => {
                self.agent = Some(agent);
                self.notice("Агент запущено. Очікуємо мережеві адреси та сусідні вузли.");
            }
            Err(error) => self.notice(format!("Не вдалося запустити агент: {error:#}")),
        }
    }

    fn stop_agent(&mut self) {
        if let Some(mut agent) = self.agent.take() {
            let _ = agent.send("quit");
            self.notice("Агент зупинено.");
        }
    }

    fn send(&mut self, command: impl Into<String>) {
        let command = command.into();
        if let Some(agent) = self.agent.as_mut() {
            if let Err(error) = agent.send(&command) {
                self.notice(format!("Помилка команди: {error:#}"));
            }
        } else {
            self.notice("Спочатку запустіть агент.");
        }
    }

    fn selected_peer_id(&self) -> Option<String> {
        self.selected_peer
            .clone()
            .or_else(|| self.peers.keys().next().cloned())
    }

    fn find_agents(&mut self) {
        self.notice("Пошук у локальній мережі активний. Очікуємо mDNS-відповіді...");
        self.send("peers");
        let peers = self.peers.keys().cloned().collect::<Vec<_>>();
        for peer in peers {
            self.send(format!("resources {peer}"));
        }
    }

    fn test_connection(&mut self) {
        if let Some(peer) = self.selected_peer_id() {
            self.notice(format!("Перевіряємо зв'язок із {}...", short_peer(&peer)));
            self.send(format!("connect {peer}"));
            self.send(format!("echo {peer} swagri-connection-test"));
        } else {
            self.notice("Спочатку знайдіть та оберіть агент.");
        }
    }

    fn run_quick_test(&mut self, kind: &str) {
        let Some(peer) = self.selected_peer_id() else {
            self.notice("Спочатку знайдіть та оберіть агент.");
            return;
        };
        let command = match kind {
            "sum" => format!("sum {peer} 1 2 3 4 5"),
            "sha256" => format!("sha256 {peer} Swagri"),
            "benchmark" => format!("benchmark {peer} 1000000"),
            "info" => format!("info {peer}"),
            _ => format!("echo {peer} hello-from-swagri-debugger"),
        };
        self.send(command);
    }

    fn check_versions(&mut self) {
        let peers = self.peers.keys().cloned().collect::<Vec<_>>();
        if peers.is_empty() {
            self.notice("Немає знайдених агентів для перевірки версій.");
        }
        for peer in peers {
            self.send(format!("info {peer}"));
        }
    }

    fn manual_dial(&mut self) {
        let address = self.dial_address.trim().to_owned();
        if address.is_empty() {
            self.notice("Вставте повну multiaddress віддаленого агента.");
        } else {
            self.send(format!("dial {address}"));
            self.notice("Ручне підключення запущено.");
        }
    }

    fn install_update(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Swagri installer", &["exe"])
            .set_title("Оберіть Swagri Debugger Setup")
            .pick_file()
        else {
            return;
        };

        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !filename.starts_with("swagri-debugger-setup") || !filename.ends_with(".exe") {
            self.notice("Оберіть офіційний Swagri-Debugger-Setup-x64.exe.");
            return;
        }

        self.stop_agent();
        match Command::new(&path).spawn() {
            Ok(_) => {
                self.notice("Інсталятор оновлення запущено. Debugger закривається.");
                self.close_requested = true;
            }
            Err(error) => self.notice(format!("Не вдалося запустити інсталятор: {error}")),
        }
    }

    fn request_peer_update(&mut self, peer: String) {
        self.send(format!("trust {peer}"));
        self.send(format!("download-update {peer}"));
        self.requested_updates.insert(peer.clone());
        self.notice(format!(
            "Peer {} додано до довірених. Запитуємо підписане оновлення…",
            short_peer(&peer)
        ));
    }

    fn apply_ready_update(&mut self, peer: String, version: String, replacement: PathBuf) {
        if !self.updater_path.is_file() {
            self.notice(format!(
                "Не знайдено {}. Встановіть повний пакет Swagri 0.3 або новіший.",
                self.updater_path.display()
            ));
            return;
        }
        let args_path = replacement.with_extension("debugger-restart.json");
        if let Err(error) = fs::write(&args_path, b"[]") {
            self.notice(format!("Не вдалося підготувати updater: {error}"));
            return;
        }
        self.stop_agent();
        let backup = self.agent_path.with_extension("previous.exe");
        let mut command = Command::new(&self.updater_path);
        command
            .arg("--target")
            .arg(&self.agent_path)
            .arg("--replacement")
            .arg(&replacement)
            .arg("--backup")
            .arg(backup)
            .arg("--restart-args")
            .arg(args_path)
            .arg("--no-restart");
        configure_child_process(&mut command);
        match command.spawn() {
            Ok(child) => {
                self.updater_child = Some(child);
                self.notice(format!(
                    "Оновлення агента до {version} від {} перевірено. Замінюємо файл…",
                    short_peer(&peer)
                ));
            }
            Err(error) => self.notice(format!("Не вдалося запустити updater: {error}")),
        }
    }

    fn poll_agent(&mut self) {
        while let Ok(line) = self.output_rx.try_recv() {
            self.handle_output(&line);
            push_bounded(&mut self.logs, line, MAX_LOG_LINES);
        }

        let exit = self
            .agent
            .as_mut()
            .and_then(|agent| agent.child.try_wait().ok().flatten());
        if let Some(status) = exit {
            self.agent = None;
            self.notice(format!("Агент завершив роботу: {status}."));
        }

        if let Some((peer, version, path)) = self.ready_update.take() {
            self.apply_ready_update(peer, version, path);
        }

        let updater_exit = self
            .updater_child
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten());
        if let Some(status) = updater_exit {
            self.updater_child = None;
            if status.success() {
                self.notice("Агент оновлено. Запускаємо нову версію…");
                self.start_agent();
            } else {
                self.notice(format!(
                    "Updater завершився з помилкою {status}. Попередня версія мала бути відновлена."
                ));
            }
        }
    }

    fn handle_output(&mut self, line: &str) {
        let Some(payload) = line.strip_prefix("SWAGRI_EVENT\t") else {
            return;
        };
        let fields = payload.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["STARTED", peer_id, version, ..] => {
                self.local_peer_id = Some((*peer_id).into());
                self.local_version = (*version).into();
            }
            ["LISTENING", address, ..] => {
                if !self.listen_addresses.iter().any(|item| item == address) {
                    self.listen_addresses.push((*address).into());
                }
            }
            ["PEER_DISCOVERED", peer_id, address, ..] => {
                let peer = self.peers.entry((*peer_id).into()).or_default();
                peer.state = PeerState::Discovered;
                if !peer.addresses.iter().any(|item| item == address) {
                    peer.addresses.push((*address).into());
                }
                if self.selected_peer.is_none() {
                    self.selected_peer = Some((*peer_id).into());
                }
                self.notice(format!("Знайдено агент {}.", short_peer(peer_id)));
            }
            ["PEER_CONNECTING", peer_id, ..] => {
                self.peers.entry((*peer_id).into()).or_default().state = PeerState::Connecting;
            }
            ["PEER_CONNECTED", peer_id, ..] => {
                self.peers.entry((*peer_id).into()).or_default().state = PeerState::Connected;
                self.notice(format!("З'єднання з {} встановлено.", short_peer(peer_id)));
                self.send(format!("resources {peer_id}"));
            }
            ["PEER_DISCONNECTED", peer_id, ..] => {
                self.peers.entry((*peer_id).into()).or_default().state = PeerState::Disconnected;
            }
            ["PEER_FAILED", peer_id, error, ..] => {
                let peer = self.peers.entry((*peer_id).into()).or_default();
                peer.state = PeerState::Failed;
                peer.last_message = (*error).into();
                self.notice(format!(
                    "Мережеве підключення до {} не вдалося. Перевірте Firewall.",
                    short_peer(peer_id)
                ));
            }
            ["PEER_VERSION", peer_id, version, ..] => {
                let peer = self.peers.entry((*peer_id).into()).or_default();
                peer.version = Some((*version).into());
                let should_update = self.automatic_peer_updates
                    && peer.trusted_for_updates
                    && is_newer(version, &self.local_version)
                    && !self.requested_updates.contains(*peer_id);
                self.notice(format!(
                    "Агент {} має версію {version}.",
                    short_peer(peer_id)
                ));
                if should_update {
                    self.request_peer_update((*peer_id).into());
                }
            }
            ["LOCAL_RESOURCES", _, values @ ..] => {
                if let Some(resources) = parse_resource_view(values) {
                    self.local_resources = Some(resources);
                }
            }
            ["PEER_RESOURCES", peer_id, values @ ..] => {
                if let Some(resources) = parse_resource_view(values) {
                    self.peers.entry((*peer_id).into()).or_default().resources = Some(resources);
                }
            }
            ["UPDATE_TRUSTED", peer_id, ..] => {
                self.peers
                    .entry((*peer_id).into())
                    .or_default()
                    .trusted_for_updates = true;
            }
            ["UPDATE_UNTRUSTED", peer_id, ..] => {
                self.peers
                    .entry((*peer_id).into())
                    .or_default()
                    .trusted_for_updates = false;
            }
            ["UPDATE_PROGRESS", peer_id, received, total, ..] => {
                if let (Ok(received), Ok(total)) = (received.parse(), total.parse()) {
                    self.peers
                        .entry((*peer_id).into())
                        .or_default()
                        .update_progress = Some((received, total));
                }
            }
            ["UPDATE_READY", peer_id, version, path, ..] => {
                self.ready_update =
                    Some(((*peer_id).into(), (*version).into(), PathBuf::from(path)));
            }
            ["UPDATE_FAILED", peer_id, error, ..] => {
                self.requested_updates.remove(*peer_id);
                self.notice(format!(
                    "P2P-оновлення від {} не виконано: {error}",
                    short_peer(peer_id)
                ));
            }
            ["TASK_RESULT", peer_id, _, duration, ..] => {
                self.completed_tasks += 1;
                let peer = self.peers.entry((*peer_id).into()).or_default();
                peer.state = PeerState::Connected;
                peer.last_message = format!("Тест успішний ({duration} ms)");
                self.notice(format!("Тест зв'язку з {} успішний.", short_peer(peer_id)));
            }
            ["TASK_FAILED", peer_id, error, ..] => {
                let peer = self.peers.entry((*peer_id).into()).or_default();
                peer.state = PeerState::Failed;
                peer.last_message = (*error).into();
                self.notice(format!(
                    "Не вдалося підключитися до {}: {error}",
                    short_peer(peer_id)
                ));
            }
            _ => {}
        }
    }

    fn notice(&mut self, message: impl Into<String>) {
        push_bounded(&mut self.notices, message.into(), 100);
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

    fn draw_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Swagri Debugger");
            let (status, color) = if self.agent.is_some() {
                ("АГЕНТ ПРАЦЮЄ", Color32::from_rgb(70, 210, 130))
            } else {
                ("АГЕНТ ЗУПИНЕНО", Color32::from_rgb(230, 100, 90))
            };
            ui.label(RichText::new(status).color(color).strong());
            ui.separator();
            ui.label(format!("Версія {}", self.local_version));
            ui.separator();
            ui.label(format!("Успішних відповідей: {}", self.completed_tasks));
        });
    }

    fn draw_main_actions(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("Швидкий старт").strong().size(18.0));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(self.agent.is_none(), egui::Button::new("▶ Запустити агент"))
                    .clicked()
                {
                    self.start_agent();
                }
                if ui
                    .add_enabled(self.agent.is_some(), egui::Button::new("■ Зупинити агент"))
                    .clicked()
                {
                    self.stop_agent();
                }
                if ui
                    .add_enabled(self.agent.is_some(), egui::Button::new("⌕ Знайти агентів"))
                    .clicked()
                {
                    self.find_agents();
                }
                if ui
                    .add_enabled(
                        self.agent.is_some() && !self.peers.is_empty(),
                        egui::Button::new("✓ Тест зв'язку"),
                    )
                    .clicked()
                {
                    self.test_connection();
                }
                if ui
                    .add_enabled(
                        !self.peers.is_empty(),
                        egui::Button::new("↻ Оновити ресурси й версії"),
                    )
                    .clicked()
                {
                    self.check_versions();
                }
            });
            ui.label(self.notices.back().map(String::as_str).unwrap_or("Готово."));
        });
    }

    fn draw_peers(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(format!("Знайдені агенти ({})", self.peers.len())).strong());
        if self.peers.is_empty() {
            ui.label("Поки нікого не знайдено. Запустіть агенти в одній приватній мережі.");
            return;
        }

        egui::Grid::new("peer_table")
            .striped(true)
            .num_columns(5)
            .show(ui, |ui| {
                ui.strong("Вибір");
                ui.strong("Peer ID");
                ui.strong("Стан");
                ui.strong("Версія");
                ui.strong("Остання перевірка");
                ui.end_row();

                for (peer_id, peer) in &self.peers {
                    ui.radio_value(&mut self.selected_peer, Some(peer_id.clone()), "");
                    ui.label(short_peer(peer_id)).on_hover_text(peer_id);
                    ui.label(RichText::new(peer.state.label()).color(peer.state.color()));
                    ui.label(peer.version.as_deref().unwrap_or("невідомо"));
                    ui.label(&peer.last_message);
                    ui.end_row();
                }
            });

        ui.horizontal_wrapped(|ui| {
            ui.label("Швидкі тести:");
            for (label, kind) in [
                ("Echo", "echo"),
                ("Sum", "sum"),
                ("SHA-256", "sha256"),
                ("CPU benchmark", "benchmark"),
                ("Версія агента", "info"),
            ] {
                if ui.button(label).clicked() {
                    self.run_quick_test(kind);
                }
            }
        });

        ui.add_space(4.0);
        ui.label(RichText::new("Ресурси рою").strong());
        let recommended = self
            .peers
            .iter()
            .filter(|(_, peer)| peer.state == PeerState::Connected)
            .filter_map(|(peer_id, peer)| {
                peer.resources
                    .as_ref()
                    .map(|resources| (peer_id, resources.effective_cpu_score))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(peer_id, _)| peer_id.clone());

        egui::ScrollArea::horizontal().show(ui, |ui| {
            egui::Grid::new("resource_table")
                .striped(true)
                .num_columns(8)
                .show(ui, |ui| {
                    ui.strong("Агент");
                    ui.strong("Процесор");
                    ui.strong("Ядра");
                    ui.strong("CPU пристрою");
                    ui.strong("Вільна RAM");
                    ui.strong("Swagri");
                    ui.strong("Ефективна сила");
                    ui.strong("Вибір");
                    ui.end_row();

                    for (peer_id, peer) in &self.peers {
                        ui.label(short_peer(peer_id)).on_hover_text(peer_id);
                        if let Some(resources) = &peer.resources {
                            ui.label(short_cpu(&resources.cpu_brand))
                                .on_hover_text(format!(
                                    "{} / {} {} · калібрована сила {:.1}",
                                    resources.os,
                                    resources.arch,
                                    resources.cpu_brand,
                                    resources.calibrated_cpu_score
                                ));
                            ui.label(format!(
                                "{}/{}",
                                resources.physical_cores, resources.logical_cores
                            ));
                            ui.label(format!("{:.1}%", resources.host_cpu_percent));
                            ui.label(format_gib(resources.available_memory_bytes))
                                .on_hover_text(format!(
                                    "Потенційно доступно Swagri: {} з {}",
                                    format_gib(resources.allocatable_memory_bytes),
                                    format_gib(resources.total_memory_bytes)
                                ));
                            ui.label(format!(
                                "CPU {:.1}% · RAM {} · задач {}",
                                resources.agent_cpu_percent,
                                format_gib(resources.agent_memory_bytes),
                                resources.active_tasks
                            ))
                            .on_hover_text(format!(
                                "Ліміти: CPU {:.0}%, RAM {:.0}% · знімок {}",
                                resources.cpu_limit_percent,
                                resources.memory_limit_percent,
                                resources.observed_at_unix_ms
                            ));
                            ui.label(format!("{:.1}", resources.effective_cpu_score));
                            if recommended.as_deref() == Some(peer_id.as_str()) {
                                ui.label(
                                    RichText::new("рекомендовано")
                                        .color(Color32::from_rgb(70, 210, 130))
                                        .strong(),
                                );
                            } else {
                                ui.label("—");
                            }
                        } else {
                            ui.label("очікуємо дані");
                            for _ in 0..6 {
                                ui.label("—");
                            }
                        }
                        ui.end_row();
                    }
                });
        });

        if let Some(peer_id) = recommended
            && let Some(resources) = self
                .peers
                .get(&peer_id)
                .and_then(|peer| peer.resources.as_ref())
        {
            ui.small(format!(
                "Зараз найкращий кандидат: {} — ефективна сила {:.1} із каліброваних {:.1}; CPU пристрою зайнятий на {:.1}%, доступно {} RAM.",
                short_peer(&peer_id),
                resources.effective_cpu_score,
                resources.calibrated_cpu_score,
                resources.host_cpu_percent,
                format_gib(resources.allocatable_memory_bytes)
            ));
        }
    }

    fn draw_metrics(&self, ui: &mut egui::Ui) {
        let used_gib = self.system.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let total_gib = self.system.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;
        let cpu = self.cpu_history.back().map_or(0.0, |sample| sample[1]);
        ui.label(format!(
            "CPU {cpu:.1}%    Пам'ять {used_gib:.2}/{total_gib:.2} GiB"
        ));
        if let Some(resources) = &self.local_resources {
            ui.small(format!(
                "Локальний Agent: CPU {:.1}% · RAM {} · доступна рою ефективна сила {:.1}",
                resources.agent_cpu_percent,
                format_gib(resources.agent_memory_bytes),
                resources.effective_cpu_score
            ));
        }
        Plot::new("host_metrics")
            .height(130.0)
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

    fn draw_updates(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("Встановлена версія: {}", self.local_version));
        let selected = self.selected_peer_id();
        let selected_info = selected.as_ref().and_then(|peer_id| {
            self.peers.get(peer_id).map(|peer| {
                (
                    peer.version.clone(),
                    peer.trusted_for_updates,
                    peer.update_progress,
                )
            })
        });
        if let Some((Some(version), trusted, progress)) = &selected_info {
            let newer = is_newer(version, &self.local_version);
            let text = if newer {
                format!("На вибраному агенті новіша версія {version} — її можна передати через рій")
            } else {
                format!("Версія вибраного агента: {version}")
            };
            ui.label(RichText::new(text).color(if newer {
                Color32::from_rgb(255, 190, 70)
            } else {
                Color32::LIGHT_GRAY
            }));
            ui.label(if *trusted {
                "Цей Peer ID довірений для підписаних оновлень."
            } else {
                "Peer ще не довірений. Перше оновлення потребує явного підтвердження."
            });
            if let Some((received, total)) = progress {
                let fraction = *received as f32 / (*total).max(1) as f32;
                ui.add(egui::ProgressBar::new(fraction).show_percentage());
            }
        }
        ui.checkbox(
            &mut self.automatic_peer_updates,
            "Автоматично оновлюватися від уже довірених агентів",
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Перевірити версії агентів").clicked() {
                self.check_versions();
            }
            if ui
                .add_enabled(
                    selected.is_some() && self.agent.is_some(),
                    egui::Button::new("Довіряти й оновити агента через P2P"),
                )
                .clicked()
                && let Some(peer) = selected.clone()
            {
                self.request_peer_update(peer);
            }
            if ui
                .add_enabled(
                    selected.is_some() && self.agent.is_some(),
                    egui::Button::new("Прибрати довіру"),
                )
                .clicked()
                && let Some(peer) = selected.clone()
            {
                self.send(format!("untrust {peer}"));
            }
            if ui.button("Відкрити сторінку завантажень").clicked()
                && let Err(error) = webbrowser::open(DOWNLOADS_URL)
            {
                self.notice(format!("Не вдалося відкрити браузер: {error}"));
            }
            if ui.button("Оновити з інсталятора...").clicked() {
                self.install_update();
            }
        });
        ui.small(
            "P2P-оновлення замінює лише Agent. Пакет підписується сталою Ed25519-ідентичністю джерела, приймається тільки від довіреного Peer ID і додатково перевіряється SHA-256. Debugger оновлюється окремо інсталятором.",
        );
    }

    fn draw_debug_tools(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("configuration")
            .num_columns(2)
            .show(ui, |ui| {
                ui.label("Ім'я вузла");
                ui.text_edit_singleline(&mut self.node_name);
                ui.end_row();
                ui.label("Agent binary");
                path_editor(ui, &mut self.agent_path);
                ui.end_row();
                ui.label("Updater binary");
                path_editor(ui, &mut self.updater_path);
                ui.end_row();
                ui.label("Identity");
                path_editor(ui, &mut self.identity_path);
                ui.end_row();
                ui.label("Listen address");
                ui.text_edit_singleline(&mut self.listen_address);
                ui.end_row();
                ui.label("Максимум CPU для Swagri");
                ui.add_enabled(
                    self.agent.is_none(),
                    egui::Slider::new(&mut self.max_cpu_percent, 5.0..=100.0).suffix("%"),
                );
                ui.end_row();
                ui.label("Максимум RAM для Swagri");
                ui.add_enabled(
                    self.agent.is_none(),
                    egui::Slider::new(&mut self.max_memory_percent, 5.0..=100.0).suffix("%"),
                );
                ui.end_row();
            });
        ui.small("Ліміти визначають, скільки вільного ресурсу агент може запропонувати рою. Змінюються після перезапуску агента; це ще не жорсткі обмеження ОС.");
        ui.label(format!(
            "Local Peer ID: {}",
            self.local_peer_id.as_deref().unwrap_or("ще не отримано")
        ));
        if !self.listen_addresses.is_empty() {
            ui.label(format!("Listening: {}", self.listen_addresses.join(", ")));
        }
        ui.horizontal(|ui| {
            ui.label("Ручна multiaddress:");
            ui.text_edit_singleline(&mut self.dial_address);
            if ui.button("Підключити").clicked() {
                self.manual_dial();
            }
        });
        if ui.button("Скопіювати команду Windows Firewall").clicked() {
            ui.ctx().copy_text(firewall_command(&self.agent_path));
            self.notice("Команду скопійовано. Запустіть PowerShell від адміністратора.");
        }
        ui.checkbox(&mut self.show_raw_console, "Показати технічний термінал");

        if self.show_raw_console {
            egui::ScrollArea::vertical()
                .id_salt("agent_log")
                .stick_to_bottom(true)
                .max_height(210.0)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.monospace(line);
                    }
                });
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 80.0).max(100.0);
                let response = ui.add_sized(
                    [width, 24.0],
                    egui::TextEdit::singleline(&mut self.command)
                        .hint_text("help, peers, connect, dial, echo..."),
                );
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.button("Надіслати").clicked() || enter {
                    let command = self.command.trim().to_owned();
                    if !command.is_empty() {
                        self.send(command);
                        self.command.clear();
                    }
                }
            });
        }
    }
}

impl eframe::App for DebuggerApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_agent();
        self.refresh_metrics();

        egui::CentralPanel::default().show(root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.draw_header(ui);
                ui.separator();
                self.draw_main_actions(ui);
                ui.separator();
                self.draw_peers(ui);
                ui.separator();
                self.draw_metrics(ui);
                ui.separator();
                ui.collapsing("Оновлення", |ui| self.draw_updates(ui));
                ui.collapsing(
                    "Розширені налаштування та debug",
                    |ui| self.draw_debug_tools(ui),
                );
            });
        });

        if self.close_requested {
            root.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        root.ctx().request_repaint_after(Duration::from_millis(250));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_agent(
    path: &Path,
    node_name: &str,
    identity_path: &Path,
    listen_address: &str,
    updater_path: &Path,
    max_cpu_percent: f32,
    max_memory_percent: f32,
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
        .args(["--update-policy", "manual", "--updater"])
        .arg(updater_path)
        .arg("--max-cpu-percent")
        .arg(max_cpu_percent.to_string())
        .arg("--max-memory-percent")
        .arg(max_memory_percent.to_string())
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
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn configure_child_process(_command: &mut Command) {}

fn sibling_agent_path() -> PathBuf {
    let name = if cfg!(windows) {
        "swagri-agent.exe"
    } else {
        "swagri-agent"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn sibling_updater_path() -> PathBuf {
    let name = if cfg!(windows) {
        "swagri-updater.exe"
    } else {
        "swagri-updater"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn default_identity_path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Swagri")
        .join("debugger.key")
}

fn default_node_name() -> String {
    std::env::var("COMPUTERNAME")
        .map(|name| format!("debugger-{name}"))
        .unwrap_or_else(|_| "swagri-debugger".into())
}

fn path_editor(ui: &mut egui::Ui, path: &mut PathBuf) {
    let mut text = path.to_string_lossy().into_owned();
    if ui.text_edit_singleline(&mut text).changed() {
        *path = PathBuf::from(text);
    }
}

fn firewall_command(agent_path: &Path) -> String {
    format!(
        "New-NetFirewallRule -DisplayName \"Swagri Agent QUIC\" -Direction Inbound -Program \"{}\" -Protocol UDP -Action Allow -Profile Private",
        agent_path.display()
    )
}

fn short_peer(peer: &str) -> String {
    if peer.len() > 20 {
        format!("{}…{}", &peer[..10], &peer[peer.len() - 6..])
    } else {
        peer.into()
    }
}

fn short_cpu(cpu: &str) -> String {
    let shortened = cpu.chars().take(30).collect::<String>();
    if shortened.chars().count() < cpu.chars().count() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

fn format_gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
}

fn parse_resource_view(values: &[&str]) -> Option<ResourceView> {
    if values.len() < 17 {
        return None;
    }
    Some(ResourceView {
        observed_at_unix_ms: values[0].parse().ok()?,
        os: values[1].into(),
        arch: values[2].into(),
        cpu_brand: values[3].into(),
        physical_cores: values[4].parse().ok()?,
        logical_cores: values[5].parse().ok()?,
        total_memory_bytes: values[6].parse().ok()?,
        available_memory_bytes: values[7].parse().ok()?,
        host_cpu_percent: values[8].parse().ok()?,
        agent_cpu_percent: values[9].parse().ok()?,
        agent_memory_bytes: values[10].parse().ok()?,
        active_tasks: values[11].parse().ok()?,
        cpu_limit_percent: values[12].parse().ok()?,
        memory_limit_percent: values[13].parse().ok()?,
        allocatable_memory_bytes: values[14].parse().ok()?,
        calibrated_cpu_score: values[15].parse().ok()?,
        effective_cpu_score: values[16].parse().ok()?,
    })
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

fn push_bounded<T>(items: &mut VecDeque<T>, item: T, limit: usize) {
    if items.len() == limit {
        items.pop_front();
    }
    items.push_back(item);
}

fn push_sample(history: &mut VecDeque<[f64; 2]>, sample: [f64; 2]) {
    push_bounded(history, sample, MAX_METRIC_SAMPLES);
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
    fn detects_newer_semantic_version() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn firewall_rule_targets_agent_binary() {
        let command = firewall_command(&PathBuf::from("C:\\Swagri\\swagri-agent.exe"));
        assert!(command.contains("swagri-agent.exe"));
        assert!(command.contains("-Protocol UDP"));
    }

    #[test]
    fn parses_resource_event_payload() {
        let fields = [
            "123",
            "windows",
            "x86_64",
            "Example CPU",
            "8",
            "16",
            "34359738368",
            "17179869184",
            "72.5",
            "2.5",
            "104857600",
            "1",
            "75",
            "50",
            "8589934592",
            "400.0",
            "110.0",
        ];
        let resources = parse_resource_view(&fields).expect("valid resource event");
        assert_eq!(resources.logical_cores, 16);
        assert_eq!(resources.allocatable_memory_bytes, 8_589_934_592);
        assert_eq!(resources.effective_cpu_score, 110.0);
    }
}
