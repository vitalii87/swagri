use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fs::{self, File},
    hint::black_box,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use futures::StreamExt;
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, SwarmBuilder, identify, identity, mdns, ping,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use swagri_core::{
    MAX_DISTRIBUTED_MATRIX_SIZE, MAX_MATRIX_CHUNK_ROWS, MAX_UPDATE_BYTES, NODE_PROTOCOL_VERSION,
    REMOTE_CPU_MINIMUM_GAIN, ResourceSnapshot, SignedUpdateManifest, TASK_PROTOCOL_V1, Task,
    TaskOutcome, TaskRequest, TaskResponse, TaskResult, UPDATE_CHUNK_BYTES, UPDATE_PROTOCOL_V1,
    UpdateManifest, UpdateRequest, UpdateResponse, choose_cpu_placement, effective_cpu_score,
};
use swagri_executor::execute;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

const REMOTE_RESOURCE_MAX_AGE: Duration = Duration::from_secs(20);

#[derive(Debug, Parser)]
#[command(
    name = "swagri-agent",
    version,
    about = "Run a lightweight headless Swagri agent"
)]
struct Args {
    /// Human-readable name shown in identify metadata and local output.
    #[arg(long, default_value = "swagri-agent")]
    name: String,

    /// File containing the persistent Ed25519 node identity.
    #[arg(long)]
    identity: Option<PathBuf>,

    /// QUIC multiaddress on which to accept peer connections.
    #[arg(long, default_value = "/ip4/0.0.0.0/udp/0/quic-v1")]
    listen: Multiaddr,

    /// Explicit peer address to dial. May be provided more than once.
    #[arg(long)]
    dial: Vec<Multiaddr>,

    /// Timeout applied to outbound task requests.
    #[arg(long, default_value_t = 30)]
    request_timeout_seconds: u64,

    /// Apply updates only from explicitly trusted peer IDs.
    #[arg(long, value_enum, default_value_t = UpdatePolicy::Manual)]
    update_policy: UpdatePolicy,

    /// File containing peer IDs trusted to provide signed updates.
    #[arg(long)]
    update_trust: Option<PathBuf>,

    /// Directory used for verified update downloads.
    #[arg(long)]
    update_staging: Option<PathBuf>,

    /// Separate updater helper used to replace the running executable.
    #[arg(long)]
    updater: Option<PathBuf>,

    /// Keep running when standard input is closed (used after self-update).
    #[arg(long)]
    daemon: bool,

    /// Maximum share of total CPU capacity Swagri may advertise as usable.
    #[arg(long, default_value_t = 75.0)]
    max_cpu_percent: f32,

    /// Maximum share of physical memory Swagri may advertise as usable.
    #[arg(long, default_value_t = 50.0)]
    max_memory_percent: f32,

    /// Seconds between lightweight local resource samples.
    #[arg(long, default_value_t = 5)]
    resource_poll_seconds: u64,

    /// File containing the cached one-time CPU calibration.
    #[arg(long)]
    calibration: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum UpdatePolicy {
    Disabled,
    Manual,
    Automatic,
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "BehaviourEvent")]
struct Behaviour {
    mdns: mdns::tokio::Behaviour,
    request_response: request_response::cbor::Behaviour<TaskRequest, TaskResponse>,
    updates: request_response::cbor::Behaviour<UpdateRequest, UpdateResponse>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
}

#[derive(Debug)]
enum BehaviourEvent {
    Mdns(mdns::Event),
    RequestResponse(request_response::Event<TaskRequest, TaskResponse>),
    Update(request_response::Event<UpdateRequest, UpdateResponse>),
    Identify(Box<identify::Event>),
    Ping(ping::Event),
}

impl From<mdns::Event> for BehaviourEvent {
    fn from(event: mdns::Event) -> Self {
        Self::Mdns(event)
    }
}

impl From<request_response::Event<TaskRequest, TaskResponse>> for BehaviourEvent {
    fn from(event: request_response::Event<TaskRequest, TaskResponse>) -> Self {
        Self::RequestResponse(event)
    }
}

impl From<request_response::Event<UpdateRequest, UpdateResponse>> for BehaviourEvent {
    fn from(event: request_response::Event<UpdateRequest, UpdateResponse>) -> Self {
        Self::Update(event)
    }
}

impl From<identify::Event> for BehaviourEvent {
    fn from(event: identify::Event) -> Self {
        Self::Identify(Box::new(event))
    }
}

impl From<ping::Event> for BehaviourEvent {
    fn from(event: ping::Event) -> Self {
        Self::Ping(event)
    }
}

struct CompletedResponse {
    channel: request_response::ResponseChannel<TaskResponse>,
    response: TaskResponse,
    track_task: bool,
}

struct CompletedLocalTask {
    peer: PeerId,
    response: TaskResponse,
    matrix_job: Option<String>,
    matrix_worker: Option<MatrixWorker>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatrixWorker {
    Local,
    Remote(PeerId),
}

struct OutboundTaskMeta {
    id: String,
    tracked: bool,
    matrix_job: Option<String>,
    matrix_worker: Option<MatrixWorker>,
}

struct MatrixChunkPlan {
    index: u16,
    row_start: u16,
    row_end: u16,
}

struct DistributedMatrixJob {
    id: String,
    size: u16,
    total_chunks: u16,
    completed_chunks: u16,
    checksum: u64,
    started_at: Instant,
    pending: VecDeque<MatrixChunkPlan>,
    available_workers: VecDeque<MatrixWorker>,
    in_flight: u16,
}

struct PeerResourceObservation {
    snapshot: ResourceSnapshot,
    received_at: Instant,
    protocol_version: u16,
}

struct UpdateSource {
    path: PathBuf,
    signed: SignedUpdateManifest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateComponent {
    Agent,
    Debugger,
}

struct PendingDownload {
    peer: PeerId,
    component: UpdateComponent,
    signed: SignedUpdateManifest,
    path: PathBuf,
    file: File,
    received: u64,
    apply_when_ready: bool,
}

struct UpdateManager {
    policy: UpdatePolicy,
    trust_path: PathBuf,
    staging: PathBuf,
    updater: PathBuf,
    trusted: BTreeSet<PeerId>,
    source: UpdateSource,
    debugger_source: Option<UpdateSource>,
    requested: Option<(PeerId, UpdateComponent, bool)>,
    pending: Option<PendingDownload>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CalibrationCache {
    hardware_key: String,
    score: f64,
}

struct ResourceMonitor {
    system: System,
    pid: Pid,
    snapshot: ResourceSnapshot,
    active_tasks: Arc<AtomicU32>,
    ewma_host_cpu: Option<f32>,
    ewma_agent_cpu: Option<f32>,
}

impl ResourceMonitor {
    fn new(
        calibration_path: &Path,
        cpu_limit_percent: f32,
        memory_limit_percent: f32,
        active_tasks: Arc<AtomicU32>,
    ) -> Result<Self> {
        let mut system = System::new_all();
        system.refresh_cpu_usage();
        system.refresh_memory();
        let pid = Pid::from_u32(std::process::id());
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);

        let logical_cores = system.cpus().len().max(1).min(u16::MAX as usize) as u16;
        let physical_cores = System::physical_core_count()
            .unwrap_or(logical_cores as usize)
            .max(1)
            .min(u16::MAX as usize) as u16;
        let cpu_brand = system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|brand| !brand.is_empty())
            .unwrap_or_else(|| "unknown CPU".into());
        let total_memory_bytes = system.total_memory();
        let hardware_key = format!(
            "{}|{}|{}|{}",
            cpu_brand,
            logical_cores,
            total_memory_bytes,
            std::env::consts::ARCH
        );
        let calibrated_cpu_score =
            load_or_calibrate_cpu(calibration_path, &hardware_key, logical_cores)?;

        let snapshot = ResourceSnapshot {
            observed_at_unix_ms: unix_time_ms(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            cpu_brand,
            physical_cores,
            logical_cores,
            total_memory_bytes,
            available_memory_bytes: system.available_memory(),
            host_cpu_percent: system.global_cpu_usage(),
            agent_cpu_percent: 0.0,
            agent_memory_bytes: system.process(pid).map_or(0, |process| process.memory()),
            active_tasks: 0,
            cpu_limit_percent,
            memory_limit_percent,
            allocatable_memory_bytes: 0,
            calibrated_cpu_score,
            effective_cpu_score: 0.0,
            contribution_paused: false,
        };
        let mut monitor = Self {
            system,
            pid,
            snapshot,
            active_tasks,
            ewma_host_cpu: None,
            ewma_agent_cpu: None,
        };
        monitor.refresh();
        Ok(monitor)
    }

    fn refresh(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system
            .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), false);

        let host_cpu = self.system.global_cpu_usage().clamp(0.0, 100.0);
        let process_cpu = self
            .system
            .process(self.pid)
            .map_or(0.0, |process| process.cpu_usage());
        let agent_cpu =
            (process_cpu / f32::from(self.snapshot.logical_cores.max(1))).clamp(0.0, 100.0);
        let host_cpu = ewma(&mut self.ewma_host_cpu, host_cpu);
        let agent_cpu = ewma(&mut self.ewma_agent_cpu, agent_cpu);
        let agent_memory = self
            .system
            .process(self.pid)
            .map_or(0, |process| process.memory());
        let memory_policy_bytes = (self.snapshot.total_memory_bytes as f64
            * f64::from(self.snapshot.memory_limit_percent)
            / 100.0) as u64;

        self.snapshot.observed_at_unix_ms = unix_time_ms();
        self.snapshot.available_memory_bytes = self.system.available_memory();
        self.snapshot.host_cpu_percent = host_cpu;
        self.snapshot.agent_cpu_percent = agent_cpu;
        self.snapshot.agent_memory_bytes = agent_memory;
        self.snapshot.active_tasks = self.active_tasks.load(Ordering::Relaxed);
        if self.snapshot.contribution_paused {
            self.snapshot.allocatable_memory_bytes = 0;
            self.snapshot.effective_cpu_score = 0.0;
        } else {
            self.snapshot.allocatable_memory_bytes = self
                .snapshot
                .available_memory_bytes
                .min(memory_policy_bytes.saturating_sub(agent_memory));
            self.snapshot.effective_cpu_score = effective_cpu_score(
                self.snapshot.calibrated_cpu_score,
                host_cpu,
                agent_cpu,
                self.snapshot.cpu_limit_percent,
            );
        }
    }

    fn set_contribution_paused(&mut self, paused: bool) {
        self.snapshot.contribution_paused = paused;
        self.refresh();
    }
}

fn ewma(previous: &mut Option<f32>, sample: f32) -> f32 {
    let value = previous.map_or(sample, |old| old * 0.75 + sample * 0.25);
    *previous = Some(value);
    value
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn load_or_calibrate_cpu(path: &Path, hardware_key: &str, logical_cores: u16) -> Result<f64> {
    if let Ok(bytes) = fs::read(path)
        && let Ok(cache) = serde_json::from_slice::<CalibrationCache>(&bytes)
        && cache.hardware_key == hardware_key
        && cache.score.is_finite()
        && cache.score > 0.0
    {
        return Ok(cache.score);
    }

    let start = Instant::now();
    let duration = Duration::from_millis(200);
    let mut iterations = 0_u64;
    let mut value = 0x9e37_79b9_7f4a_7c15_u64;
    while start.elapsed() < duration {
        for _ in 0..4096 {
            value ^= value << 13;
            value ^= value >> 7;
            value ^= value << 17;
            iterations += 1;
        }
        black_box(value);
    }
    let score = iterations as f64 / start.elapsed().as_secs_f64() / 1_000_000.0
        * f64::from(logical_cores.max(1));
    let cache = CalibrationCache {
        hardware_key: hardware_key.into(),
        score,
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(&cache)?)
        .with_context(|| format!("could not save CPU calibration to {}", path.display()))?;
    Ok(score)
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    validate_resource_limits(&args)?;
    let identity_path = args.identity.clone().unwrap_or_else(default_identity_path);
    let keypair = load_or_create_identity(&identity_path)?;
    let local_peer_id = PeerId::from(keypair.public());
    let request_timeout = Duration::from_secs(args.request_timeout_seconds);
    let trust_path = args
        .update_trust
        .clone()
        .unwrap_or_else(|| identity_path.with_file_name("trusted-update-peers.txt"));
    let staging = args
        .update_staging
        .clone()
        .unwrap_or_else(default_update_staging);
    let updater = args.updater.clone().unwrap_or_else(default_updater_path);
    let calibration_path = args
        .calibration
        .clone()
        .unwrap_or_else(|| identity_path.with_file_name("cpu-calibration.json"));
    let restart_arguments = restart_arguments(
        &args,
        &identity_path,
        &trust_path,
        &staging,
        &updater,
        &calibration_path,
    );
    let source = build_update_source(&keypair)?;
    let debugger_source = build_debugger_update_source(&keypair)?;
    let mut updates = UpdateManager::new(
        args.update_policy,
        trust_path,
        staging,
        updater,
        source,
        debugger_source,
    )?;

    let mut swarm = build_swarm(keypair, &args.name, request_timeout)?;

    swarm
        .listen_on(args.listen.clone())
        .with_context(|| format!("could not listen on {}", args.listen))?;

    for address in &args.dial {
        info!(%address, "dialing explicit peer address");
        swarm
            .dial(address.clone())
            .with_context(|| format!("could not dial {address}"))?;
    }

    let (completed_tx, mut completed_rx) = mpsc::unbounded_channel::<CompletedResponse>();
    let (local_completed_tx, mut local_completed_rx) =
        mpsc::unbounded_channel::<CompletedLocalTask>();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut known_peers = BTreeMap::<PeerId, BTreeSet<Multiaddr>>::new();
    let mut connected_peers = BTreeSet::<PeerId>::new();
    let mut peer_resources = BTreeMap::<PeerId, PeerResourceObservation>::new();
    let mut outbound_tasks =
        HashMap::<request_response::OutboundRequestId, OutboundTaskMeta>::new();
    let mut matrix_jobs = BTreeMap::<String, DistributedMatrixJob>::new();
    let request_counter = AtomicU64::new(1);
    let active_tasks = Arc::new(AtomicU32::new(0));
    let mut resources = ResourceMonitor::new(
        &calibration_path,
        args.max_cpu_percent,
        args.max_memory_percent,
        active_tasks.clone(),
    )?;
    let mut resource_tick = tokio::time::interval(Duration::from_secs(args.resource_poll_seconds));
    resource_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stdin_closed = false;

    println!("Swagri agent '{}'", args.name);
    println!("Peer ID: {local_peer_id}");
    println!("Identity: {}", identity_path.display());
    emit_event(
        "STARTED",
        &[
            &local_peer_id.to_string(),
            env!("CARGO_PKG_VERSION"),
            &args.name,
        ],
    );
    for peer in &updates.trusted {
        emit_event("UPDATE_TRUSTED", &[&peer.to_string()]);
    }
    print_help();

    loop {
        tokio::select! {
            maybe_line = lines.next_line(), if !stdin_closed => {
                match maybe_line.context("failed to read stdin")? {
                    Some(line) => {
                        if !handle_command(
                            &line,
                            &mut swarm,
                            local_peer_id,
                            &request_counter,
                            &known_peers,
                            &mut updates,
                            &restart_arguments,
                            &mut resources,
                            &peer_resources,
                            &local_completed_tx,
                            &active_tasks,
                            &mut outbound_tasks,
                            &mut matrix_jobs,
                        ) {
                            break;
                        }
                        dispatch_matrix_jobs(
                            &mut matrix_jobs,
                            &mut swarm,
                            local_peer_id,
                            &mut outbound_tasks,
                            &local_completed_tx,
                            &active_tasks,
                        );
                    }
                    None if args.daemon => stdin_closed = true,
                    None => break,
                }
            }
            event = swarm.select_next_some() => {
                if handle_swarm_event(
                    event,
                    &mut swarm,
                    &completed_tx,
                    &mut known_peers,
                    &mut connected_peers,
                    &mut peer_resources,
                    &mut updates,
                    &restart_arguments,
                    &resources.snapshot,
                    &active_tasks,
                    &args.name,
                    &mut outbound_tasks,
                    &mut matrix_jobs,
                ) {
                    break;
                }
                dispatch_matrix_jobs(
                    &mut matrix_jobs,
                    &mut swarm,
                    local_peer_id,
                    &mut outbound_tasks,
                    &local_completed_tx,
                    &active_tasks,
                );
            }
            Some(completed) = completed_rx.recv() => {
                if completed.track_task {
                    report_task_response(local_peer_id, &completed.response);
                }
                if swarm
                    .behaviour_mut()
                    .request_response
                    .send_response(completed.channel, completed.response)
                    .is_err()
                {
                    warn!("requester disconnected before the response was sent");
                }
            }
            Some(completed) = local_completed_rx.recv() => {
                report_task_response(completed.peer, &completed.response);
                if let (Some(job_id), Some(worker)) =
                    (completed.matrix_job, completed.matrix_worker)
                {
                    complete_matrix_chunk(
                        &job_id,
                        worker,
                        &completed.response,
                        local_peer_id,
                        &mut matrix_jobs,
                    );
                    dispatch_matrix_jobs(
                        &mut matrix_jobs,
                        &mut swarm,
                        local_peer_id,
                        &mut outbound_tasks,
                        &local_completed_tx,
                        &active_tasks,
                    );
                }
            }
            _ = resource_tick.tick() => {
                resources.refresh();
                emit_resource_event("LOCAL_RESOURCES", &local_peer_id.to_string(), &resources.snapshot);
                for peer in &connected_peers {
                    submit_task(
                        &mut swarm,
                        *peer,
                        Task::NodeInfo,
                        local_peer_id,
                        &request_counter,
                        &mut outbound_tasks,
                    );
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("received shutdown signal");
                break;
            }
        }
    }

    Ok(())
}

fn validate_resource_limits(args: &Args) -> Result<()> {
    if !(1.0..=100.0).contains(&args.max_cpu_percent) {
        bail!("--max-cpu-percent must be between 1 and 100");
    }
    if !(1.0..=100.0).contains(&args.max_memory_percent) {
        bail!("--max-memory-percent must be between 1 and 100");
    }
    if args.resource_poll_seconds < 2 {
        bail!("--resource-poll-seconds must be at least 2");
    }
    Ok(())
}

fn default_identity_path() -> PathBuf {
    if let Some(local_data) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_data)
            .join("Swagri")
            .join("identity.key");
    }

    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("swagri")
            .join("identity.key");
    }

    PathBuf::from(".swagri").join("identity.key")
}

fn default_update_staging() -> PathBuf {
    default_identity_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("updates")
}

fn default_updater_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(updater_filename())))
        .unwrap_or_else(|| PathBuf::from(updater_filename()))
}

fn updater_filename() -> &'static str {
    if cfg!(windows) {
        "swagri-updater.exe"
    } else {
        "swagri-updater"
    }
}

fn debugger_filename() -> &'static str {
    if cfg!(windows) {
        "swagri-debugger.exe"
    } else {
        "swagri-debugger"
    }
}

fn build_update_source(keypair: &identity::Keypair) -> Result<UpdateSource> {
    let path = std::env::current_exe().context("could not locate running agent executable")?;
    build_signed_update_source(
        path,
        env!("CARGO_PKG_VERSION"),
        keypair,
        UpdateComponent::Agent,
    )
}

fn build_debugger_update_source(keypair: &identity::Keypair) -> Result<Option<UpdateSource>> {
    let path = std::env::current_exe()
        .context("could not locate running agent executable")?
        .with_file_name(debugger_filename());
    if !path.is_file() {
        return Ok(None);
    }
    let marker = path.with_extension("version");
    if !marker.is_file() {
        warn!(path = %marker.display(), "Debugger version marker is missing; GUI sharing disabled");
        return Ok(None);
    }
    let version = fs::read_to_string(&marker)
        .with_context(|| format!("could not read {}", marker.display()))?;
    let version = version.trim();
    Version::parse(version).context("Debugger version marker is invalid")?;
    build_signed_update_source(path, version, keypair, UpdateComponent::Debugger).map(Some)
}

fn build_signed_update_source(
    path: PathBuf,
    version: &str,
    keypair: &identity::Keypair,
    component: UpdateComponent,
) -> Result<UpdateSource> {
    let mut file = File::open(&path)
        .with_context(|| format!("could not open update source {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let manifest = UpdateManifest {
        version: version.into(),
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        size,
        sha256_hex: hex::encode(hasher.finalize()),
    };
    let signing_payload = match component {
        UpdateComponent::Agent => manifest.signing_payload(),
        UpdateComponent::Debugger => manifest.debugger_signing_payload(),
    };
    let signature = keypair
        .sign(&signing_payload)
        .context("node identity could not sign its update manifest")?;
    Ok(UpdateSource {
        path,
        signed: SignedUpdateManifest {
            manifest,
            signer_public_key: keypair.public().encode_protobuf(),
            signature,
        },
    })
}

impl UpdateManager {
    fn new(
        policy: UpdatePolicy,
        trust_path: PathBuf,
        staging: PathBuf,
        updater: PathBuf,
        source: UpdateSource,
        debugger_source: Option<UpdateSource>,
    ) -> Result<Self> {
        fs::create_dir_all(&staging)
            .with_context(|| format!("could not create update staging at {}", staging.display()))?;
        let trusted = load_trusted_peers(&trust_path)?;
        Ok(Self {
            policy,
            trust_path,
            staging,
            updater,
            trusted,
            source,
            debugger_source,
            requested: None,
            pending: None,
        })
    }

    fn persist_trust(&self) -> Result<()> {
        if let Some(parent) = self.trust_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = self
            .trusted
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&self.trust_path, format!("{text}\n"))
            .with_context(|| format!("could not save {}", self.trust_path.display()))
    }
}

fn load_trusted_peers(path: &Path) -> Result<BTreeSet<PeerId>> {
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            PeerId::from_str(line).with_context(|| format!("invalid trusted peer ID: {line}"))
        })
        .collect()
}

fn restart_arguments(
    args: &Args,
    identity: &Path,
    trust: &Path,
    staging: &Path,
    updater: &Path,
    calibration: &Path,
) -> Vec<String> {
    let mut result = vec![
        "--name".into(),
        args.name.clone(),
        "--identity".into(),
        identity.to_string_lossy().into_owned(),
        "--listen".into(),
        args.listen.to_string(),
        "--request-timeout-seconds".into(),
        args.request_timeout_seconds.to_string(),
        "--update-policy".into(),
        match args.update_policy {
            UpdatePolicy::Disabled => "disabled",
            UpdatePolicy::Manual => "manual",
            UpdatePolicy::Automatic => "automatic",
        }
        .into(),
        "--update-trust".into(),
        trust.to_string_lossy().into_owned(),
        "--update-staging".into(),
        staging.to_string_lossy().into_owned(),
        "--updater".into(),
        updater.to_string_lossy().into_owned(),
        "--max-cpu-percent".into(),
        args.max_cpu_percent.to_string(),
        "--max-memory-percent".into(),
        args.max_memory_percent.to_string(),
        "--resource-poll-seconds".into(),
        args.resource_poll_seconds.to_string(),
        "--calibration".into(),
        calibration.to_string_lossy().into_owned(),
        "--daemon".into(),
    ];
    for address in &args.dial {
        result.push("--dial".into());
        result.push(address.to_string());
    }
    result
}

fn build_swarm(
    keypair: identity::Keypair,
    node_name: &str,
    request_timeout: Duration,
) -> Result<libp2p::Swarm<Behaviour>> {
    let agent_version = format!("swagri/{} ({node_name})", env!("CARGO_PKG_VERSION"));

    Ok(SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_behaviour(move |key| {
            let local_peer_id = PeerId::from(key.public());
            let request_response = request_response::cbor::Behaviour::new(
                [(StreamProtocol::new(TASK_PROTOCOL_V1), ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(request_timeout),
            );
            let updates = request_response::cbor::Behaviour::new(
                [(
                    StreamProtocol::new(UPDATE_PROTOCOL_V1),
                    ProtocolSupport::Full,
                )],
                request_response::Config::default().with_request_timeout(request_timeout),
            );

            Ok(Behaviour {
                mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?,
                request_response,
                updates,
                identify: identify::Behaviour::new(
                    identify::Config::new("/swagri/identify/1".into(), key.public())
                        .with_agent_version(agent_version),
                ),
                ping: ping::Behaviour::default(),
            })
        })?
        .with_swarm_config(|config| config.with_idle_connection_timeout(Duration::from_secs(60)))
        .build())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .init();
}

fn load_or_create_identity(path: &Path) -> Result<identity::Keypair> {
    if path.exists() {
        let encoded = fs::read(path)
            .with_context(|| format!("could not read identity from {}", path.display()))?;
        return identity::Keypair::from_protobuf_encoding(&encoded)
            .context("identity file is not a valid libp2p keypair");
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    let keypair = identity::Keypair::generate_ed25519();
    let encoded = keypair
        .to_protobuf_encoding()
        .context("could not encode generated node identity")?;
    fs::write(path, encoded)
        .with_context(|| format!("could not save identity to {}", path.display()))?;

    Ok(keypair)
}

#[allow(clippy::too_many_arguments)]
fn handle_swarm_event(
    event: SwarmEvent<BehaviourEvent>,
    swarm: &mut libp2p::Swarm<Behaviour>,
    completed_tx: &mpsc::UnboundedSender<CompletedResponse>,
    known_peers: &mut BTreeMap<PeerId, BTreeSet<Multiaddr>>,
    connected_peers: &mut BTreeSet<PeerId>,
    peer_resources: &mut BTreeMap<PeerId, PeerResourceObservation>,
    updates: &mut UpdateManager,
    restart_arguments: &[String],
    resources: &ResourceSnapshot,
    active_tasks: &Arc<AtomicU32>,
    node_name: &str,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
) -> bool {
    let mut shutdown = false;
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            println!("Listening on {address}");
            emit_event("LISTENING", &[&address.to_string()]);
        }
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            known_peers.entry(peer_id).or_default();
            connected_peers.insert(peer_id);
            info!(peer = %peer_id, "peer connected");
            emit_event("PEER_CONNECTED", &[&peer_id.to_string()]);
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            connected_peers.remove(&peer_id);
            peer_resources.remove(&peer_id);
            info!(peer = %peer_id, ?cause, "peer disconnected");
            emit_event("PEER_DISCONNECTED", &[&peer_id.to_string()]);
        }
        SwarmEvent::OutgoingConnectionError {
            peer_id: Some(peer_id),
            error,
            ..
        } => {
            warn!(peer = %peer_id, %error, "peer connection failed");
            emit_event("PEER_FAILED", &[&peer_id.to_string(), &error.to_string()]);
        }
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
            for (peer_id, address) in peers {
                info!(peer = %peer_id, %address, "discovered peer through mDNS");
                known_peers
                    .entry(peer_id)
                    .or_default()
                    .insert(address.clone());
                emit_event(
                    "PEER_DISCOVERED",
                    &[&peer_id.to_string(), &address.to_string()],
                );
                swarm.add_peer_address(peer_id, address);
                if !swarm.is_connected(&peer_id) {
                    match swarm.dial(peer_id) {
                        Ok(()) => emit_event("PEER_CONNECTING", &[&peer_id.to_string()]),
                        Err(error) => debug!(peer = %peer_id, %error, "automatic dial deferred"),
                    }
                }
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
            for (peer_id, address) in peers {
                debug!(peer = %peer_id, %address, "mDNS peer address expired");
                if let Some(addresses) = known_peers.get_mut(&peer_id) {
                    addresses.remove(&address);
                }
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(event)) => {
            handle_request_response(
                event,
                completed_tx,
                *swarm.local_peer_id(),
                node_name,
                resources,
                active_tasks,
                peer_resources,
                outbound_tasks,
                matrix_jobs,
            );
        }
        SwarmEvent::Behaviour(BehaviourEvent::Update(event)) => {
            shutdown = handle_update_event(event, swarm, updates, restart_arguments);
        }
        SwarmEvent::Behaviour(BehaviourEvent::Identify(event)) => {
            if let identify::Event::Received { peer_id, info, .. } = *event {
                let version = info
                    .agent_version
                    .strip_prefix("swagri/")
                    .and_then(|value| value.split_whitespace().next())
                    .unwrap_or(&info.agent_version);
                emit_event(
                    "PEER_VERSION",
                    &[
                        &peer_id.to_string(),
                        version,
                        &protocol_hint_for_version(version).to_string(),
                    ],
                );
                if updates.policy == UpdatePolicy::Automatic
                    && updates.trusted.contains(&peer_id)
                    && updates.pending.is_none()
                    && is_newer_version(version)
                {
                    begin_update_request(swarm, updates, peer_id, true);
                }
            } else {
                debug!(?event, "identify event");
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::Ping(event)) => {
            debug!(?event, "ping event");
        }
        _ => {}
    }
    shutdown
}

#[allow(clippy::too_many_arguments)]
fn handle_request_response(
    event: request_response::Event<TaskRequest, TaskResponse>,
    completed_tx: &mpsc::UnboundedSender<CompletedResponse>,
    local_peer_id: PeerId,
    node_name: &str,
    resources: &ResourceSnapshot,
    active_tasks: &Arc<AtomicU32>,
    peer_resources: &mut BTreeMap<PeerId, PeerResourceObservation>,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
) {
    match event {
        request_response::Event::Message {
            peer,
            message:
                request_response::Message::Request {
                    request, channel, ..
                },
            ..
        } => {
            info!(peer = %peer, task_id = %request.id, kind = ?request.task.kind(), "received task");
            if request.task == Task::NodeInfo {
                let response = TaskResponse::success(
                    request.id,
                    0,
                    TaskResult::NodeInfo {
                        agent_version: env!("CARGO_PKG_VERSION").into(),
                        protocol_version: NODE_PROTOCOL_VERSION,
                        node_name: node_name.into(),
                        resources: Some(resources.clone()),
                    },
                );
                let _ = completed_tx.send(CompletedResponse {
                    channel,
                    response,
                    track_task: false,
                });
                return;
            }
            if resources.contribution_paused
                && !task_allowed_while_contribution_paused(&request.task)
            {
                emit_event(
                    "INBOUND_TASK_REJECTED",
                    &[&peer.to_string(), "local contribution is paused"],
                );
                let response = TaskResponse::failure(
                    request.id,
                    0,
                    "resources_paused",
                    "the selected Agent has paused its compute contribution",
                );
                let _ = completed_tx.send(CompletedResponse {
                    channel,
                    response,
                    track_task: false,
                });
                return;
            }
            emit_task_started(
                &request.id,
                &task_description(&request.task),
                local_peer_id,
                "inbound",
            );
            let completed_tx = completed_tx.clone();
            let active_tasks = active_tasks.clone();
            active_tasks.fetch_add(1, Ordering::Relaxed);
            tokio::task::spawn_blocking(move || {
                let response = execute(request);
                active_tasks.fetch_sub(1, Ordering::Relaxed);
                let _ = completed_tx.send(CompletedResponse {
                    channel,
                    response,
                    track_task: true,
                });
            });
        }
        request_response::Event::Message {
            peer,
            message:
                request_response::Message::Response {
                    request_id,
                    response,
                },
            ..
        } => {
            let outbound = outbound_tasks.remove(&request_id);
            let is_node_info = if let TaskOutcome::Success {
                result:
                    TaskResult::NodeInfo {
                        agent_version,
                        protocol_version,
                        node_name,
                        resources,
                    },
            } = &response.outcome
            {
                emit_event(
                    "PEER_VERSION",
                    &[
                        &peer.to_string(),
                        agent_version,
                        &protocol_version.to_string(),
                    ],
                );
                if !node_name.is_empty() {
                    emit_event("PEER_NAME", &[&peer.to_string(), node_name]);
                }
                if let Some(resources) = resources {
                    emit_resource_event("PEER_RESOURCES", &peer.to_string(), resources);
                    peer_resources.insert(
                        peer,
                        PeerResourceObservation {
                            snapshot: resources.clone(),
                            received_at: Instant::now(),
                            protocol_version: *protocol_version,
                        },
                    );
                }
                true
            } else {
                false
            };
            if !is_node_info {
                report_task_response(peer, &response);
                if let Some(outbound) = outbound
                    && let (Some(job_id), Some(worker)) =
                        (outbound.matrix_job, outbound.matrix_worker)
                {
                    complete_matrix_chunk(&job_id, worker, &response, local_peer_id, matrix_jobs);
                }
            }
        }
        request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            warn!(peer = %peer, ?request_id, %error, "outbound task failed");
            if let Some(outbound) = outbound_tasks.remove(&request_id) {
                if outbound.tracked {
                    emit_event(
                        "TASK_FAILED",
                        &[&peer.to_string(), &outbound.id, "0", &error.to_string()],
                    );
                } else {
                    emit_event("PEER_POLL_FAILED", &[&peer.to_string(), &error.to_string()]);
                }
                if let Some(job_id) = outbound.matrix_job {
                    fail_matrix_job(
                        &job_id,
                        &format!("chunk connection failed: {error}"),
                        local_peer_id,
                        matrix_jobs,
                    );
                }
            } else {
                emit_event("PEER_POLL_FAILED", &[&peer.to_string(), &error.to_string()]);
            }
        }
        request_response::Event::InboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            warn!(peer = %peer, ?request_id, %error, "inbound task failed");
        }
        request_response::Event::ResponseSent {
            peer, request_id, ..
        } => {
            debug!(peer = %peer, ?request_id, "task response sent");
        }
    }
}

fn task_allowed_while_contribution_paused(task: &Task) -> bool {
    matches!(task, Task::NodeInfo | Task::Echo { .. })
}

fn report_task_response(peer: PeerId, response: &TaskResponse) {
    match &response.outcome {
        TaskOutcome::Success { result } => {
            let summary = task_result_summary(result);
            emit_event(
                "TASK_RESULT",
                &[
                    &peer.to_string(),
                    &response.id,
                    &response.duration_ms.to_string(),
                    &summary,
                ],
            );
        }
        TaskOutcome::Failure { message, .. } => {
            emit_event(
                "TASK_FAILED",
                &[
                    &peer.to_string(),
                    &response.id,
                    &response.duration_ms.to_string(),
                    message,
                ],
            );
        }
    }
    println!(
        "Result from {peer}: id={} duration={}ms outcome={:?}",
        response.id, response.duration_ms, response.outcome
    );
}

fn task_result_summary(result: &TaskResult) -> String {
    match result {
        TaskResult::MatrixMultiply { checksum, size } => {
            format!("matrix {size}x{size}, checksum {checksum}")
        }
        TaskResult::MatrixChunk {
            checksum,
            size,
            row_start,
            row_end,
        } => format!(
            "matrix {size}x{size} rows {row_start}-{}, checksum {checksum}",
            row_end.saturating_sub(1)
        ),
        TaskResult::DistributedMatrix {
            checksum,
            size,
            chunks,
        } => format!("distributed matrix {size}x{size}, {chunks} chunks, checksum {checksum}"),
        TaskResult::CpuBenchmark {
            checksum,
            iterations,
        } => format!("CPU benchmark {iterations} iterations, checksum {checksum}"),
        TaskResult::Sum { value } => format!("sum {value}"),
        TaskResult::Sha256 { digest_hex } => format!("SHA-256 {digest_hex}"),
        TaskResult::Echo { message } => format!("echo {message}"),
        TaskResult::NodeInfo { agent_version, .. } => format!("Agent {agent_version}"),
    }
}

fn begin_update_request(
    swarm: &mut libp2p::Swarm<Behaviour>,
    updates: &mut UpdateManager,
    peer: PeerId,
    apply_when_ready: bool,
) {
    begin_component_update_request(
        swarm,
        updates,
        peer,
        UpdateComponent::Agent,
        apply_when_ready,
    );
}

fn begin_debugger_update_request(
    swarm: &mut libp2p::Swarm<Behaviour>,
    updates: &mut UpdateManager,
    peer: PeerId,
) {
    begin_component_update_request(swarm, updates, peer, UpdateComponent::Debugger, false);
}

fn begin_component_update_request(
    swarm: &mut libp2p::Swarm<Behaviour>,
    updates: &mut UpdateManager,
    peer: PeerId,
    component: UpdateComponent,
    apply_when_ready: bool,
) {
    let failed_event = component_event(component, "FAILED");
    if updates.policy == UpdatePolicy::Disabled {
        emit_event(failed_event, &[&peer.to_string(), "updates are disabled"]);
        return;
    }
    if !updates.trusted.contains(&peer) {
        emit_event(
            failed_event,
            &[&peer.to_string(), "peer is not trusted for updates"],
        );
        return;
    }
    if updates.pending.is_some() || updates.requested.is_some() {
        emit_event(
            failed_event,
            &[&peer.to_string(), "another update is already active"],
        );
        return;
    }
    updates.requested = Some((peer, component, apply_when_ready));
    let request = match component {
        UpdateComponent::Agent => UpdateRequest::Manifest,
        UpdateComponent::Debugger => UpdateRequest::DebuggerManifest,
    };
    swarm.behaviour_mut().updates.send_request(&peer, request);
    emit_event(
        component_event(component, "REQUESTED"),
        &[&peer.to_string()],
    );
}

fn component_event(component: UpdateComponent, suffix: &str) -> &'static str {
    match (component, suffix) {
        (UpdateComponent::Agent, "REQUESTED") => "UPDATE_REQUESTED",
        (UpdateComponent::Agent, "PROGRESS") => "UPDATE_PROGRESS",
        (UpdateComponent::Agent, "READY") => "UPDATE_READY",
        (UpdateComponent::Agent, "FAILED") => "UPDATE_FAILED",
        (UpdateComponent::Debugger, "REQUESTED") => "DEBUGGER_UPDATE_REQUESTED",
        (UpdateComponent::Debugger, "PROGRESS") => "DEBUGGER_UPDATE_PROGRESS",
        (UpdateComponent::Debugger, "READY") => "DEBUGGER_UPDATE_READY",
        (UpdateComponent::Debugger, "FAILED") => "DEBUGGER_UPDATE_FAILED",
        _ => "UPDATE_FAILED",
    }
}

fn handle_update_event(
    event: request_response::Event<UpdateRequest, UpdateResponse>,
    swarm: &mut libp2p::Swarm<Behaviour>,
    updates: &mut UpdateManager,
    restart_arguments: &[String],
) -> bool {
    match event {
        request_response::Event::Message {
            peer,
            message:
                request_response::Message::Request {
                    request, channel, ..
                },
            ..
        } => {
            let response = serve_update_request(&request, updates);
            if swarm
                .behaviour_mut()
                .updates
                .send_response(channel, response)
                .is_err()
            {
                warn!(%peer, "update requester disconnected before response");
            }
        }
        request_response::Event::Message {
            peer,
            message: request_response::Message::Response { response, .. },
            ..
        } => match response {
            UpdateResponse::Manifest { signed } => {
                let Some((requested_peer, component, apply_when_ready)) = updates.requested.take()
                else {
                    warn!(%peer, "ignored unsolicited update manifest");
                    return false;
                };
                if requested_peer != peer {
                    emit_event(
                        component_event(component, "FAILED"),
                        &[&peer.to_string(), "unexpected update source"],
                    );
                    return false;
                }
                if let Err(error) = verify_update_manifest(peer, &signed, component) {
                    emit_event(
                        component_event(component, "FAILED"),
                        &[&peer.to_string(), &error.to_string()],
                    );
                    return false;
                }
                let component_name = match component {
                    UpdateComponent::Agent => "agent",
                    UpdateComponent::Debugger => "debugger",
                };
                let filename = format!(
                    "swagri-{component_name}-{}-{}.download",
                    signed.manifest.version, peer
                );
                let path = updates.staging.join(filename);
                let file = match File::create(&path) {
                    Ok(file) => file,
                    Err(error) => {
                        emit_event(
                            component_event(component, "FAILED"),
                            &[&peer.to_string(), &error.to_string()],
                        );
                        return false;
                    }
                };
                updates.pending = Some(PendingDownload {
                    peer,
                    component,
                    signed,
                    path,
                    file,
                    received: 0,
                    apply_when_ready,
                });
                request_next_update_chunk(swarm, updates);
            }
            UpdateResponse::Chunk { offset, data } => {
                let Some(mut pending) = updates.pending.take() else {
                    warn!(%peer, "ignored unsolicited update chunk");
                    return false;
                };
                if pending.peer != peer || offset != pending.received {
                    emit_event(
                        component_event(pending.component, "FAILED"),
                        &[&peer.to_string(), "invalid update chunk order"],
                    );
                    let _ = fs::remove_file(&pending.path);
                    return false;
                }
                if data.is_empty()
                    || pending.received.saturating_add(data.len() as u64)
                        > pending.signed.manifest.size
                {
                    emit_event(
                        component_event(pending.component, "FAILED"),
                        &[&peer.to_string(), "invalid update chunk size"],
                    );
                    let _ = fs::remove_file(&pending.path);
                    return false;
                }
                if let Err(error) = pending.file.write_all(&data) {
                    emit_event(
                        component_event(pending.component, "FAILED"),
                        &[&peer.to_string(), &error.to_string()],
                    );
                    let _ = fs::remove_file(&pending.path);
                    return false;
                }
                pending.received += data.len() as u64;
                emit_event(
                    component_event(pending.component, "PROGRESS"),
                    &[
                        &peer.to_string(),
                        &pending.received.to_string(),
                        &pending.signed.manifest.size.to_string(),
                    ],
                );
                if pending.received == pending.signed.manifest.size {
                    if let Err(error) = pending.file.flush().and_then(|_| pending.file.sync_all()) {
                        emit_event(
                            component_event(pending.component, "FAILED"),
                            &[&peer.to_string(), &error.to_string()],
                        );
                        let _ = fs::remove_file(&pending.path);
                        return false;
                    }
                    if let Err(error) = verify_download(&pending.path, &pending.signed.manifest) {
                        emit_event(
                            component_event(pending.component, "FAILED"),
                            &[&peer.to_string(), &error.to_string()],
                        );
                        let _ = fs::remove_file(&pending.path);
                        return false;
                    }
                    emit_event(
                        component_event(pending.component, "READY"),
                        &[
                            &peer.to_string(),
                            &pending.signed.manifest.version,
                            &pending.path.to_string_lossy(),
                        ],
                    );
                    if pending.component == UpdateComponent::Agent && pending.apply_when_ready {
                        return schedule_self_update(
                            updates,
                            &pending.path,
                            restart_arguments,
                            peer,
                        );
                    }
                } else {
                    updates.pending = Some(pending);
                    request_next_update_chunk(swarm, updates);
                }
            }
            UpdateResponse::Error { message } => {
                let component = updates
                    .requested
                    .as_ref()
                    .map_or(UpdateComponent::Agent, |(_, component, _)| *component);
                updates.requested = None;
                updates.pending = None;
                emit_event(
                    component_event(component, "FAILED"),
                    &[&peer.to_string(), &message],
                );
            }
        },
        request_response::Event::OutboundFailure { peer, error, .. } => {
            let component = updates
                .requested
                .as_ref()
                .map(|(_, component, _)| *component)
                .or_else(|| updates.pending.as_ref().map(|pending| pending.component))
                .unwrap_or(UpdateComponent::Agent);
            updates.requested = None;
            if let Some(pending) = updates.pending.take() {
                let _ = fs::remove_file(pending.path);
            }
            emit_event(
                component_event(component, "FAILED"),
                &[&peer.to_string(), &error.to_string()],
            );
        }
        request_response::Event::InboundFailure { peer, error, .. } => {
            warn!(%peer, %error, "inbound update request failed");
        }
        request_response::Event::ResponseSent { .. } => {}
    }
    false
}

fn serve_update_request(request: &UpdateRequest, updates: &UpdateManager) -> UpdateResponse {
    match request {
        UpdateRequest::Manifest => UpdateResponse::Manifest {
            signed: updates.source.signed.clone(),
        },
        UpdateRequest::DebuggerManifest => updates.debugger_source.as_ref().map_or_else(
            || UpdateResponse::Error {
                message: "this peer does not have a Debugger binary to share".into(),
            },
            |source| UpdateResponse::Manifest {
                signed: source.signed.clone(),
            },
        ),
        UpdateRequest::Chunk {
            version,
            offset,
            length,
        } => serve_update_chunk(&updates.source, version, *offset, *length),
        UpdateRequest::DebuggerChunk {
            version,
            offset,
            length,
        } => updates.debugger_source.as_ref().map_or_else(
            || UpdateResponse::Error {
                message: "this peer does not have a Debugger binary to share".into(),
            },
            |source| serve_update_chunk(source, version, *offset, *length),
        ),
    }
}

fn serve_update_chunk(
    source: &UpdateSource,
    version: &str,
    offset: u64,
    length: u32,
) -> UpdateResponse {
    if version != source.signed.manifest.version {
        return UpdateResponse::Error {
            message: "requested update version is unavailable".into(),
        };
    }
    if offset >= source.signed.manifest.size {
        return UpdateResponse::Error {
            message: "update offset is outside the package".into(),
        };
    }
    let remaining = source.signed.manifest.size - offset;
    let length = u64::from(length.min(UPDATE_CHUNK_BYTES)).min(remaining) as usize;
    let result = (|| -> Result<Vec<u8>> {
        let mut file = File::open(&source.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0; length];
        file.read_exact(&mut data)?;
        Ok(data)
    })();
    match result {
        Ok(data) => UpdateResponse::Chunk { offset, data },
        Err(error) => UpdateResponse::Error {
            message: error.to_string(),
        },
    }
}

fn request_next_update_chunk(swarm: &mut libp2p::Swarm<Behaviour>, updates: &UpdateManager) {
    if let Some(pending) = &updates.pending {
        let request = match pending.component {
            UpdateComponent::Agent => UpdateRequest::Chunk {
                version: pending.signed.manifest.version.clone(),
                offset: pending.received,
                length: UPDATE_CHUNK_BYTES,
            },
            UpdateComponent::Debugger => UpdateRequest::DebuggerChunk {
                version: pending.signed.manifest.version.clone(),
                offset: pending.received,
                length: UPDATE_CHUNK_BYTES,
            },
        };
        swarm
            .behaviour_mut()
            .updates
            .send_request(&pending.peer, request);
    }
}

fn verify_update_manifest(
    peer: PeerId,
    signed: &SignedUpdateManifest,
    component: UpdateComponent,
) -> Result<()> {
    let manifest = &signed.manifest;
    if manifest.target_os != std::env::consts::OS || manifest.target_arch != std::env::consts::ARCH
    {
        bail!("update target does not match this device");
    }
    if manifest.size == 0 || manifest.size > MAX_UPDATE_BYTES {
        bail!("update package size is outside the allowed range");
    }
    if Version::parse(&manifest.version).is_err() {
        bail!("offered update version is invalid");
    }
    if component == UpdateComponent::Agent && !is_newer_version(&manifest.version) {
        bail!("offered version is not newer than the running agent");
    }
    let public = identity::PublicKey::try_decode_protobuf(&signed.signer_public_key)
        .context("update signer key is invalid")?;
    if PeerId::from_public_key(&public) != peer {
        bail!("update signature key does not match the connected peer");
    }
    let signing_payload = match component {
        UpdateComponent::Agent => manifest.signing_payload(),
        UpdateComponent::Debugger => manifest.debugger_signing_payload(),
    };
    if !public.verify(&signing_payload, &signed.signature) {
        bail!("update manifest signature is invalid");
    }
    Ok(())
}

fn is_newer_version(version: &str) -> bool {
    Version::parse(version)
        .ok()
        .zip(Version::parse(env!("CARGO_PKG_VERSION")).ok())
        .is_some_and(|(remote, local)| remote > local)
}

fn protocol_hint_for_version(version: &str) -> u16 {
    match Version::parse(version).ok() {
        Some(version) if version >= Version::new(0, 10, 0) => NODE_PROTOCOL_VERSION,
        Some(version) if version >= Version::new(0, 7, 0) => 3,
        _ => 2,
    }
}

fn verify_download(path: &Path, manifest: &UpdateManifest) -> Result<()> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() != manifest.size {
        bail!("downloaded update size does not match its manifest");
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hex::encode(hasher.finalize()) != manifest.sha256_hex {
        bail!("downloaded update SHA-256 does not match its manifest");
    }
    Ok(())
}

fn schedule_self_update(
    updates: &UpdateManager,
    replacement: &Path,
    restart_arguments: &[String],
    peer: PeerId,
) -> bool {
    if !updates.updater.is_file() {
        emit_event(
            "UPDATE_FAILED",
            &[
                &peer.to_string(),
                &format!("updater helper not found: {}", updates.updater.display()),
            ],
        );
        return false;
    }
    let target = &updates.source.path;
    let backup = target.with_extension("previous.exe");
    let args_path = updates
        .staging
        .join(format!("restart-{}.json", std::process::id()));
    let result = (|| -> Result<()> {
        fs::write(&args_path, serde_json::to_vec(restart_arguments)?)?;
        Command::new(&updates.updater)
            .arg("--target")
            .arg(target)
            .arg("--replacement")
            .arg(replacement)
            .arg("--backup")
            .arg(&backup)
            .arg("--restart-args")
            .arg(&args_path)
            .spawn()
            .context("could not start updater helper")?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            emit_event(
                "UPDATE_APPLYING",
                &[&peer.to_string(), &target.to_string_lossy()],
            );
            true
        }
        Err(error) => {
            emit_event("UPDATE_FAILED", &[&peer.to_string(), &error.to_string()]);
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    line: &str,
    swarm: &mut libp2p::Swarm<Behaviour>,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    known_peers: &BTreeMap<PeerId, BTreeSet<Multiaddr>>,
    updates: &mut UpdateManager,
    restart_arguments: &[String],
    resources: &mut ResourceMonitor,
    peer_resources: &BTreeMap<PeerId, PeerResourceObservation>,
    local_completed_tx: &mpsc::UnboundedSender<CompletedLocalTask>,
    active_tasks: &Arc<AtomicU32>,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return true;
    }

    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or_default();

    match command {
        "help" => print_help(),
        "id" => println!("{local_peer_id}"),
        "peers" => print_peers(known_peers),
        "trusted" => {
            if updates.trusted.is_empty() {
                println!("No peers are trusted for updates.");
            }
            for peer in &updates.trusted {
                println!("Trusted update peer: {peer}");
                emit_event("UPDATE_TRUSTED", &[&peer.to_string()]);
            }
        }
        "trust" => match parse_peer(&mut parts) {
            Ok(peer) => {
                updates.trusted.insert(peer);
                match updates.persist_trust() {
                    Ok(()) => {
                        println!("Trusted {peer} for signed agent updates.");
                        emit_event("UPDATE_TRUSTED", &[&peer.to_string()]);
                    }
                    Err(error) => println!("Could not save update trust: {error:#}"),
                }
            }
            Err(error) => println!("Invalid trust command: {error:#}"),
        },
        "untrust" => match parse_peer(&mut parts) {
            Ok(peer) => {
                updates.trusted.remove(&peer);
                match updates.persist_trust() {
                    Ok(()) => {
                        println!("Removed update trust for {peer}.");
                        emit_event("UPDATE_UNTRUSTED", &[&peer.to_string()]);
                    }
                    Err(error) => println!("Could not save update trust: {error:#}"),
                }
            }
            Err(error) => println!("Invalid untrust command: {error:#}"),
        },
        "update" => match parse_peer(&mut parts) {
            Ok(peer) => begin_update_request(swarm, updates, peer, true),
            Err(error) => println!("Invalid update command: {error:#}"),
        },
        "download-update" => match parse_peer(&mut parts) {
            Ok(peer) => begin_update_request(swarm, updates, peer, false),
            Err(error) => println!("Invalid update command: {error:#}"),
        },
        "download-debugger-update" => match parse_peer(&mut parts) {
            Ok(peer) => begin_debugger_update_request(swarm, updates, peer),
            Err(error) => println!("Invalid Debugger update command: {error:#}"),
        },
        "apply-update" => {
            let peer = parse_peer(&mut parts);
            let path = parts.next().map(PathBuf::from);
            match (peer, path) {
                (Ok(peer), Some(path)) if path.is_file() => {
                    return !schedule_self_update(updates, &path, restart_arguments, peer);
                }
                _ => println!("apply-update requires a peer ID and verified update path"),
            }
        }
        "connect" => {
            match parse_peer(&mut parts).and_then(|peer| {
                swarm
                    .dial(peer)
                    .context("could not start peer connection")?;
                Ok(peer)
            }) {
                Ok(peer) => {
                    println!("Connecting to {peer}...");
                    emit_event("PEER_CONNECTING", &[&peer.to_string()]);
                }
                Err(error) => println!("Connection failed: {error:#}"),
            }
        }
        "dial" => {
            let result = parts
                .next()
                .context("dial requires a multiaddress")
                .and_then(|value| value.parse::<Multiaddr>().context("invalid multiaddress"))
                .and_then(|address| {
                    swarm
                        .dial(address.clone())
                        .with_context(|| format!("could not dial {address}"))?;
                    Ok(address)
                });
            match result {
                Ok(address) => println!("Dialing {address}..."),
                Err(error) => println!("Dial failed: {error:#}"),
            }
        }
        "quit" | "exit" => return false,
        "pause-resources" => {
            resources.set_contribution_paused(true);
            println!("Local compute contribution is paused.");
            emit_event("CONTRIBUTION_STATE", &["paused"]);
            emit_resource_event(
                "LOCAL_RESOURCES",
                &local_peer_id.to_string(),
                &resources.snapshot,
            );
        }
        "resume-resources" => {
            resources.set_contribution_paused(false);
            println!("Local compute contribution is enabled.");
            emit_event("CONTRIBUTION_STATE", &["enabled"]);
            emit_resource_event(
                "LOCAL_RESOURCES",
                &local_peer_id.to_string(),
                &resources.snapshot,
            );
        }
        "local-resources" => {
            println!("{:#?}", resources.snapshot);
            emit_resource_event(
                "LOCAL_RESOURCES",
                &local_peer_id.to_string(),
                &resources.snapshot,
            );
        }
        "info" | "resources" => {
            let result = parse_peer(&mut parts).map(|peer| (peer, Task::NodeInfo));
            submit_parsed(
                result,
                swarm,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        "echo" => {
            let result = parse_peer(&mut parts).and_then(|peer| {
                let message = parts.collect::<Vec<_>>().join(" ");
                if message.is_empty() {
                    bail!("echo requires a message");
                }
                Ok((peer, Task::Echo { message }))
            });
            submit_parsed(
                result,
                swarm,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        "sum" => {
            let result = parse_peer(&mut parts).and_then(|peer| {
                let values = parts
                    .map(|value| {
                        value
                            .parse::<f64>()
                            .with_context(|| format!("invalid number: {value}"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if values.is_empty() {
                    bail!("sum requires at least one number");
                }
                Ok((peer, Task::Sum { values }))
            });
            submit_parsed(
                result,
                swarm,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        "sha256" => {
            let result = parse_peer(&mut parts).map(|peer| {
                let text = parts.collect::<Vec<_>>().join(" ");
                (peer, Task::Sha256 { text })
            });
            submit_parsed(
                result,
                swarm,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        "benchmark" => {
            let result = parse_peer(&mut parts).and_then(|peer| {
                let iterations = parts
                    .next()
                    .context("benchmark requires an iteration count")?
                    .parse::<u64>()
                    .context("iteration count must be an integer")?;
                Ok((peer, Task::CpuBenchmark { iterations }))
            });
            submit_parsed(
                result,
                swarm,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        "matrix" => {
            let result = parse_peer(&mut parts).and_then(|peer| {
                let size = parts
                    .next()
                    .context("matrix requires a side length")?
                    .parse::<u16>()
                    .context("matrix size must be an integer")?;
                Ok((peer, Task::MatrixMultiply { size }))
            });
            submit_parsed(
                result,
                swarm,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        "auto-benchmark" => {
            let result = parts
                .next()
                .context("auto-benchmark requires an iteration count")
                .and_then(|value| {
                    value
                        .parse::<u64>()
                        .context("iteration count must be an integer")
                });
            match result {
                Ok(iterations) => place_cpu_task(
                    Task::CpuBenchmark { iterations },
                    2,
                    swarm,
                    local_peer_id,
                    request_counter,
                    &resources.snapshot,
                    peer_resources,
                    local_completed_tx,
                    active_tasks,
                    outbound_tasks,
                ),
                Err(error) => println!("Invalid command: {error:#}"),
            }
        }
        "auto-matrix" => {
            let result = parts
                .next()
                .context("auto-matrix requires a side length")
                .and_then(|value| {
                    value
                        .parse::<u16>()
                        .context("matrix size must be an integer")
                });
            match result {
                Ok(size) => place_cpu_task(
                    Task::MatrixMultiply { size },
                    NODE_PROTOCOL_VERSION,
                    swarm,
                    local_peer_id,
                    request_counter,
                    &resources.snapshot,
                    peer_resources,
                    local_completed_tx,
                    active_tasks,
                    outbound_tasks,
                ),
                Err(error) => println!("Invalid command: {error:#}"),
            }
        }
        "distributed-matrix" => {
            let result = parts
                .next()
                .context("distributed-matrix requires a side length")
                .and_then(|value| {
                    value
                        .parse::<u16>()
                        .context("matrix size must be an integer")
                })
                .and_then(|size| {
                    let chunk_rows = parts
                        .next()
                        .map(|value| {
                            value
                                .parse::<u16>()
                                .context("chunk row count must be an integer")
                        })
                        .transpose()?
                        .unwrap_or(96);
                    Ok((size, chunk_rows))
                });
            match result {
                Ok((size, chunk_rows)) => start_distributed_matrix(
                    size,
                    chunk_rows,
                    local_peer_id,
                    request_counter,
                    &resources.snapshot,
                    peer_resources,
                    matrix_jobs,
                ),
                Err(error) => println!("Invalid command: {error:#}"),
            }
        }
        _ => println!("Unknown command. Type 'help'."),
    }

    true
}

fn parse_peer<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Result<PeerId> {
    let value = parts.next().context("missing peer ID")?;
    PeerId::from_str(value).with_context(|| format!("invalid peer ID: {value}"))
}

fn submit_parsed(
    parsed: Result<(PeerId, Task)>,
    swarm: &mut libp2p::Swarm<Behaviour>,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
) {
    match parsed {
        Ok((peer, task)) => {
            if let Err(error) = task.validate() {
                println!("Task rejected locally: {error}");
                return;
            }

            submit_task(
                swarm,
                peer,
                task,
                local_peer_id,
                request_counter,
                outbound_tasks,
            );
        }
        Err(error) => println!("Invalid command: {error:#}"),
    }
}

fn submit_task(
    swarm: &mut libp2p::Swarm<Behaviour>,
    peer: PeerId,
    task: Task,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
) {
    let sequence = request_counter.fetch_add(1, Ordering::Relaxed);
    let id = format!("{local_peer_id}-{sequence}");
    let task_kind = task.kind();
    let description = task_description(&task);
    let tracked = task != Task::NodeInfo;
    let request_id = swarm.behaviour_mut().request_response.send_request(
        &peer,
        TaskRequest {
            id: id.clone(),
            task,
        },
    );
    outbound_tasks.insert(
        request_id,
        OutboundTaskMeta {
            id: id.clone(),
            tracked,
            matrix_job: None,
            matrix_worker: None,
        },
    );
    if tracked {
        emit_task_started(&id, &description, peer, "outbound");
    }
    debug!(%peer, %id, ?task_kind, "submitted task");
}

#[allow(clippy::too_many_arguments)]
fn place_cpu_task(
    task: Task,
    minimum_protocol_version: u16,
    swarm: &mut libp2p::Swarm<Behaviour>,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    resources: &ResourceSnapshot,
    peer_resources: &BTreeMap<PeerId, PeerResourceObservation>,
    local_completed_tx: &mpsc::UnboundedSender<CompletedLocalTask>,
    active_tasks: &Arc<AtomicU32>,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
) {
    if let Err(error) = task.validate() {
        println!("Task rejected locally: {error}");
        return;
    }
    let task_kind = format!("{:?}", task.kind());
    let description = task_description(&task);

    let candidates = peer_resources
        .iter()
        .filter(|(_, observation)| observation.received_at.elapsed() <= REMOTE_RESOURCE_MAX_AGE)
        .filter(|(_, observation)| observation.protocol_version >= minimum_protocol_version)
        .filter(|(_, observation)| {
            !observation.snapshot.contribution_paused
                && observation.snapshot.effective_cpu_score > 0.0
        })
        .map(|(peer, observation)| (*peer, observation.snapshot.effective_cpu_score))
        .collect::<Vec<_>>();
    let scores = candidates
        .iter()
        .map(|(_, score)| *score)
        .collect::<Vec<_>>();
    let decision = choose_cpu_placement(
        resources.effective_cpu_score,
        &scores,
        REMOTE_CPU_MINIMUM_GAIN,
    );

    if let Some(index) = decision.remote_candidate_index {
        let peer = candidates[index].0;
        emit_placement_decision("remote", peer, decision, candidates.len(), &task_kind);
        println!(
            "Placement: remote {peer} (local {:.1}, remote {:.1}, required {:.1})",
            decision.local_score, decision.selected_score, decision.minimum_remote_score
        );
        submit_task(
            swarm,
            peer,
            task,
            local_peer_id,
            request_counter,
            outbound_tasks,
        );
        return;
    }

    if resources.contribution_paused {
        let sequence = request_counter.fetch_add(1, Ordering::Relaxed);
        let id = format!("{local_peer_id}-blocked-{sequence}");
        emit_task_started(&id, &description, local_peer_id, "scheduler");
        let response = TaskResponse::failure(
            id,
            0,
            "no_eligible_resources",
            "local contribution is paused and no compatible remote Agent is available",
        );
        emit_event(
            "PLACEMENT_UNAVAILABLE",
            &[&task_kind, &candidates.len().to_string()],
        );
        println!(
            "Placement unavailable: local contribution is paused and no remote candidate is eligible"
        );
        let _ = local_completed_tx.send(CompletedLocalTask {
            peer: local_peer_id,
            response,
            matrix_job: None,
            matrix_worker: None,
        });
        return;
    }

    emit_placement_decision(
        "local",
        local_peer_id,
        decision,
        candidates.len(),
        &task_kind,
    );
    println!(
        "Placement: local {local_peer_id} (local {:.1}, required remote {:.1})",
        decision.local_score, decision.minimum_remote_score
    );
    submit_local_task(
        task,
        local_peer_id,
        request_counter,
        local_completed_tx,
        active_tasks,
    );
}

fn submit_local_task(
    task: Task,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    local_completed_tx: &mpsc::UnboundedSender<CompletedLocalTask>,
    active_tasks: &Arc<AtomicU32>,
) {
    let sequence = request_counter.fetch_add(1, Ordering::Relaxed);
    let id = format!("{local_peer_id}-local-{sequence}");
    let description = task_description(&task);
    let request = TaskRequest {
        id: id.clone(),
        task,
    };
    emit_task_started(&id, &description, local_peer_id, "local");
    let local_completed_tx = local_completed_tx.clone();
    let active_tasks = active_tasks.clone();
    active_tasks.fetch_add(1, Ordering::Relaxed);
    tokio::task::spawn_blocking(move || {
        let response = execute(request);
        active_tasks.fetch_sub(1, Ordering::Relaxed);
        let _ = local_completed_tx.send(CompletedLocalTask {
            peer: local_peer_id,
            response,
            matrix_job: None,
            matrix_worker: None,
        });
    });
}

fn emit_task_started(id: &str, description: &str, executor: PeerId, direction: &str) {
    emit_event(
        "TASK_STARTED",
        &[id, description, &executor.to_string(), direction],
    );
}

fn task_description(task: &Task) -> String {
    match task {
        Task::NodeInfo => "Node info".into(),
        Task::Echo { .. } => "Echo".into(),
        Task::Sum { values } => format!("Sum ({} values)", values.len()),
        Task::Sha256 { .. } => "SHA-256".into(),
        Task::CpuBenchmark { iterations } => format!("CPU benchmark ({iterations} iterations)"),
        Task::MatrixMultiply { size } => format!("Matrix {size}x{size}"),
        Task::MatrixChunk {
            size,
            row_start,
            row_end,
        } => format!("Matrix {size}x{size}, rows {row_start}..{row_end}"),
    }
}

fn start_distributed_matrix(
    size: u16,
    chunk_rows: u16,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    resources: &ResourceSnapshot,
    peer_resources: &BTreeMap<PeerId, PeerResourceObservation>,
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
) {
    if !(swagri_core::MIN_MATRIX_SIZE..=MAX_DISTRIBUTED_MATRIX_SIZE).contains(&size) {
        println!(
            "Distributed matrix size must be between {} and {MAX_DISTRIBUTED_MATRIX_SIZE}",
            swagri_core::MIN_MATRIX_SIZE
        );
        return;
    }
    if !(1..=MAX_MATRIX_CHUNK_ROWS).contains(&chunk_rows) {
        println!("Chunk rows must be between 1 and {MAX_MATRIX_CHUNK_ROWS}");
        return;
    }

    let mut workers = Vec::<(MatrixWorker, f64)>::new();
    if !resources.contribution_paused && resources.effective_cpu_score > 0.0 {
        workers.push((MatrixWorker::Local, resources.effective_cpu_score));
    }
    workers.extend(
        peer_resources
            .iter()
            .filter(|(_, observation)| observation.received_at.elapsed() <= REMOTE_RESOURCE_MAX_AGE)
            .filter(|(_, observation)| observation.protocol_version >= NODE_PROTOCOL_VERSION)
            .filter(|(_, observation)| {
                !observation.snapshot.contribution_paused
                    && observation.snapshot.effective_cpu_score > 0.0
            })
            .map(|(peer, observation)| {
                (
                    MatrixWorker::Remote(*peer),
                    observation.snapshot.effective_cpu_score,
                )
            }),
    );
    workers.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let sequence = request_counter.fetch_add(1, Ordering::Relaxed);
    let id = format!("{local_peer_id}-distributed-{sequence}");
    let total_chunks = size.div_ceil(chunk_rows);
    let description = format!("Distributed Matrix {size}x{size} ({total_chunks} chunks)");
    emit_event(
        "TASK_STARTED",
        &[&id, &description, "swarm", "orchestrator"],
    );

    if workers.is_empty() {
        let response = TaskResponse::failure(
            id,
            0,
            "no_eligible_resources",
            "no compatible Agent currently offers compute resources",
        );
        report_task_response(local_peer_id, &response);
        return;
    }

    let pending = (0..total_chunks)
        .map(|index| {
            let row_start = index * chunk_rows;
            MatrixChunkPlan {
                index,
                row_start,
                row_end: (row_start + chunk_rows).min(size),
            }
        })
        .collect::<VecDeque<_>>();
    let available_workers = workers
        .into_iter()
        .map(|(worker, _)| worker)
        .collect::<VecDeque<_>>();
    emit_event(
        "MATRIX_PLAN",
        &[
            &id,
            &size.to_string(),
            &total_chunks.to_string(),
            &available_workers.len().to_string(),
        ],
    );
    matrix_jobs.insert(
        id.clone(),
        DistributedMatrixJob {
            id,
            size,
            total_chunks,
            completed_chunks: 0,
            checksum: 0,
            started_at: Instant::now(),
            pending,
            available_workers,
            in_flight: 0,
        },
    );
}

struct MatrixDispatch {
    job_id: String,
    task_id: String,
    task: Task,
    worker: MatrixWorker,
}

fn dispatch_matrix_jobs(
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
    swarm: &mut libp2p::Swarm<Behaviour>,
    local_peer_id: PeerId,
    outbound_tasks: &mut HashMap<request_response::OutboundRequestId, OutboundTaskMeta>,
    local_completed_tx: &mpsc::UnboundedSender<CompletedLocalTask>,
    active_tasks: &Arc<AtomicU32>,
) {
    let mut dispatches = Vec::new();
    for job in matrix_jobs.values_mut() {
        while let (Some(worker), Some(chunk)) =
            (job.available_workers.pop_front(), job.pending.pop_front())
        {
            job.in_flight += 1;
            dispatches.push(MatrixDispatch {
                job_id: job.id.clone(),
                task_id: format!("{}-chunk-{}", job.id, chunk.index + 1),
                task: Task::MatrixChunk {
                    size: job.size,
                    row_start: chunk.row_start,
                    row_end: chunk.row_end,
                },
                worker,
            });
        }
    }

    for dispatch in dispatches {
        let description = task_description(&dispatch.task);
        match dispatch.worker {
            MatrixWorker::Local => {
                emit_task_started(&dispatch.task_id, &description, local_peer_id, "local");
                let request = TaskRequest {
                    id: dispatch.task_id,
                    task: dispatch.task,
                };
                let local_completed_tx = local_completed_tx.clone();
                let active_tasks = active_tasks.clone();
                let job_id = dispatch.job_id;
                active_tasks.fetch_add(1, Ordering::Relaxed);
                tokio::task::spawn_blocking(move || {
                    let response = execute(request);
                    active_tasks.fetch_sub(1, Ordering::Relaxed);
                    let _ = local_completed_tx.send(CompletedLocalTask {
                        peer: local_peer_id,
                        response,
                        matrix_job: Some(job_id),
                        matrix_worker: Some(MatrixWorker::Local),
                    });
                });
            }
            MatrixWorker::Remote(peer) => {
                emit_task_started(&dispatch.task_id, &description, peer, "outbound");
                let request_id = swarm.behaviour_mut().request_response.send_request(
                    &peer,
                    TaskRequest {
                        id: dispatch.task_id.clone(),
                        task: dispatch.task,
                    },
                );
                outbound_tasks.insert(
                    request_id,
                    OutboundTaskMeta {
                        id: dispatch.task_id,
                        tracked: true,
                        matrix_job: Some(dispatch.job_id),
                        matrix_worker: Some(MatrixWorker::Remote(peer)),
                    },
                );
            }
        }
    }
}

fn complete_matrix_chunk(
    job_id: &str,
    worker: MatrixWorker,
    response: &TaskResponse,
    local_peer_id: PeerId,
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
) {
    let result = match &response.outcome {
        TaskOutcome::Success {
            result:
                TaskResult::MatrixChunk {
                    checksum,
                    size,
                    row_start: _,
                    row_end: _,
                },
        } => Some((*checksum, *size)),
        TaskOutcome::Success { .. } => None,
        TaskOutcome::Failure { message, .. } => {
            fail_matrix_job(
                job_id,
                &format!("chunk failed: {message}"),
                local_peer_id,
                matrix_jobs,
            );
            return;
        }
    };
    let Some((checksum, result_size)) = result else {
        fail_matrix_job(
            job_id,
            "worker returned an unexpected result type",
            local_peer_id,
            matrix_jobs,
        );
        return;
    };

    let mut finished = None;
    if let Some(job) = matrix_jobs.get_mut(job_id) {
        if result_size != job.size {
            fail_matrix_job(
                job_id,
                "worker returned a result for a different matrix size",
                local_peer_id,
                matrix_jobs,
            );
            return;
        }
        job.in_flight = job.in_flight.saturating_sub(1);
        job.available_workers.push_back(worker);
        job.completed_chunks += 1;
        job.checksum ^= checksum;
        emit_event(
            "TASK_PROGRESS",
            &[
                &job.id,
                &job.completed_chunks.to_string(),
                &job.total_chunks.to_string(),
                "matrix chunks completed",
            ],
        );
        if job.completed_chunks == job.total_chunks {
            finished = Some((
                job.id.clone(),
                job.size,
                job.total_chunks,
                job.checksum,
                job.started_at.elapsed().as_millis() as u64,
            ));
        }
    }

    if let Some((id, size, chunks, checksum, duration_ms)) = finished {
        matrix_jobs.remove(&id);
        let response = TaskResponse::success(
            id,
            duration_ms,
            TaskResult::DistributedMatrix {
                checksum,
                size,
                chunks,
            },
        );
        report_task_response(local_peer_id, &response);
    }
}

fn fail_matrix_job(
    job_id: &str,
    message: &str,
    local_peer_id: PeerId,
    matrix_jobs: &mut BTreeMap<String, DistributedMatrixJob>,
) {
    let Some(job) = matrix_jobs.remove(job_id) else {
        return;
    };
    let response = TaskResponse::failure(
        job.id,
        job.started_at.elapsed().as_millis() as u64,
        "distributed_matrix_failed",
        message,
    );
    report_task_response(local_peer_id, &response);
}

fn emit_placement_decision(
    target_kind: &str,
    target_peer: PeerId,
    decision: swagri_core::CpuPlacementDecision,
    candidate_count: usize,
    task_kind: &str,
) {
    emit_event(
        "PLACEMENT_DECISION",
        &[
            target_kind,
            &target_peer.to_string(),
            &format!("{:.3}", decision.local_score),
            &format!("{:.3}", decision.selected_score),
            &format!("{:.3}", decision.minimum_remote_score),
            &candidate_count.to_string(),
            task_kind,
        ],
    );
}

fn print_peers(peers: &BTreeMap<PeerId, BTreeSet<Multiaddr>>) {
    if peers.is_empty() {
        println!("No peers discovered yet.");
        return;
    }

    for (peer, addresses) in peers {
        println!("{peer}");
        for address in addresses {
            println!("  {address}");
        }
    }
}

fn print_help() {
    println!(
        "Commands:\n\
         help                              Show this help\n\
         id                                Print this node's peer ID\n\
         peers                             List discovered or connected peers\n\
         trusted                           List peers trusted for updates\n\
         trust <peer-id>                   Trust a peer identity for signed updates\n\
         untrust <peer-id>                 Remove update trust\n\
         update <peer-id>                  Download, verify, apply, and restart\n\
         download-update <peer-id>         Download and verify without applying\n\
         download-debugger-update <peer>   Download and verify the peer Debugger\n\
         connect <peer-id>                 Connect using a discovered address\n\
         dial <multiaddr>                  Connect using an explicit address\n\
         pause-resources                   Reject new compute work on this Agent\n\
         resume-resources                  Offer local compute resources again\n\
         local-resources                   Show this device resource snapshot\n\
         info|resources <peer-id>          Read remote version and resources\n\
         echo <peer-id> <text>             Return text from the remote node\n\
         sum <peer-id> <numbers...>         Sum finite numbers remotely\n\
         sha256 <peer-id> <text>            Hash text remotely\n\
         benchmark <peer-id> <iterations>   Run bounded synthetic CPU work\n\
         matrix <peer-id> <size>            Multiply deterministic square matrices remotely\n\
         auto-benchmark <iterations>        Place CPU work locally or on a stronger peer\n\
         auto-matrix <size>                 Smart-place deterministic matrix work\n\
         distributed-matrix <size> [rows]   Split matrix work across eligible Agents\n\
         quit                              Stop the node"
    );
}

fn emit_event(kind: &str, fields: &[&str]) {
    print!("SWAGRI_EVENT\t{kind}");
    for field in fields {
        print!("\t{}", field.replace(['\t', '\r', '\n'], " "));
    }
    println!();
}

fn emit_resource_event(kind: &str, peer: &str, resources: &ResourceSnapshot) {
    let fields = vec![
        peer.to_owned(),
        resources.observed_at_unix_ms.to_string(),
        resources.os.clone(),
        resources.arch.clone(),
        resources.cpu_brand.clone(),
        resources.physical_cores.to_string(),
        resources.logical_cores.to_string(),
        resources.total_memory_bytes.to_string(),
        resources.available_memory_bytes.to_string(),
        format!("{:.2}", resources.host_cpu_percent),
        format!("{:.2}", resources.agent_cpu_percent),
        resources.agent_memory_bytes.to_string(),
        resources.active_tasks.to_string(),
        format!("{:.2}", resources.cpu_limit_percent),
        format!("{:.2}", resources.memory_limit_percent),
        resources.allocatable_memory_bytes.to_string(),
        format!("{:.3}", resources.calibrated_cpu_score),
        format!("{:.3}", resources.effective_cpu_score),
        resources.contribution_paused.to_string(),
    ];
    let refs = fields.iter().map(String::as_str).collect::<Vec<_>>();
    emit_event(kind, &refs);
}

#[cfg(test)]
mod tests {
    use libp2p::multiaddr::Protocol;
    use swagri_core::{TaskOutcome, TaskResult};
    use tokio::time::timeout;

    use super::*;

    fn signed_manifest(keypair: &identity::Keypair, version: &str) -> SignedUpdateManifest {
        let manifest = UpdateManifest {
            version: version.into(),
            target_os: std::env::consts::OS.into(),
            target_arch: std::env::consts::ARCH.into(),
            size: 64,
            sha256_hex: "00".repeat(32),
        };
        SignedUpdateManifest {
            signature: keypair.sign(&manifest.signing_payload()).unwrap(),
            signer_public_key: keypair.public().encode_protobuf(),
            manifest,
        }
    }

    #[test]
    fn accepts_only_manifest_signed_by_connected_peer() {
        let signer = identity::Keypair::generate_ed25519();
        let peer = PeerId::from(signer.public());
        let signed = signed_manifest(&signer, "999.0.0");
        assert!(verify_update_manifest(peer, &signed, UpdateComponent::Agent).is_ok());

        let attacker = identity::Keypair::generate_ed25519();
        let attacker_peer = PeerId::from(attacker.public());
        assert!(verify_update_manifest(attacker_peer, &signed, UpdateComponent::Agent).is_err());
    }

    #[test]
    fn rejects_tampered_manifest() {
        let signer = identity::Keypair::generate_ed25519();
        let peer = PeerId::from(signer.public());
        let mut signed = signed_manifest(&signer, "999.0.0");
        signed.manifest.size += 1;
        assert!(verify_update_manifest(peer, &signed, UpdateComponent::Agent).is_err());
    }

    #[test]
    fn agent_signature_cannot_be_replayed_as_debugger_update() {
        let signer = identity::Keypair::generate_ed25519();
        let peer = PeerId::from(signer.public());
        let signed = signed_manifest(&signer, "999.0.0");

        assert!(verify_update_manifest(peer, &signed, UpdateComponent::Debugger).is_err());
    }

    #[test]
    fn protocol_hint_preserves_legacy_and_requires_0_10_for_chunks() {
        assert_eq!(protocol_hint_for_version("0.6.0"), 2);
        assert_eq!(protocol_hint_for_version("0.7.0"), 3);
        assert_eq!(protocol_hint_for_version("0.9.0"), 3);
        assert_eq!(protocol_hint_for_version("0.10.0"), NODE_PROTOCOL_VERSION);
        assert_eq!(protocol_hint_for_version("invalid"), 2);
    }

    #[test]
    fn paused_contribution_keeps_control_plane_available() {
        assert!(task_allowed_while_contribution_paused(&Task::NodeInfo));
        assert!(task_allowed_while_contribution_paused(&Task::Echo {
            message: "ping".into(),
        }));
        assert!(!task_allowed_while_contribution_paused(
            &Task::CpuBenchmark { iterations: 1 }
        ));
        assert!(!task_allowed_while_contribution_paused(
            &Task::MatrixMultiply { size: 16 }
        ));
        assert!(!task_allowed_while_contribution_paused(
            &Task::MatrixChunk {
                size: 16,
                row_start: 0,
                row_end: 8,
            }
        ));
    }

    #[test]
    fn task_descriptions_include_workload_size() {
        assert_eq!(
            task_description(&Task::MatrixMultiply { size: 320 }),
            "Matrix 320x320"
        );
        assert_eq!(
            task_description(&Task::CpuBenchmark {
                iterations: 1_000_000,
            }),
            "CPU benchmark (1000000 iterations)"
        );
        assert_eq!(
            task_description(&Task::MatrixChunk {
                size: 768,
                row_start: 96,
                row_end: 192,
            }),
            "Matrix 768x768, rows 96..192"
        );
    }

    #[tokio::test]
    async fn two_nodes_execute_tasks_in_both_directions() -> Result<()> {
        timeout(Duration::from_secs(20), async {
            let mut alpha = build_swarm(
                identity::Keypair::generate_ed25519(),
                "alpha-test",
                Duration::from_secs(5),
            )?;
            let mut beta = build_swarm(
                identity::Keypair::generate_ed25519(),
                "beta-test",
                Duration::from_secs(5),
            )?;
            let alpha_id = *alpha.local_peer_id();
            let beta_id = *beta.local_peer_id();

            alpha.listen_on(
                "/ip4/127.0.0.1/udp/0/quic-v1"
                    .parse()
                    .expect("valid test listen address"),
            )?;

            let alpha_address = loop {
                if let SwarmEvent::NewListenAddr { address, .. } = alpha.select_next_some().await {
                    break address.with(Protocol::P2p(alpha_id));
                }
            };

            beta.dial(alpha_address)?;
            let mut beta_request_sent = false;
            let mut alpha_request_sent = false;

            loop {
                tokio::select! {
                    event = alpha.select_next_some() => {
                        if let SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(
                            request_response::Event::Message { message, .. }
                        )) = event {
                            match message {
                                request_response::Message::Request { request, channel, .. } => {
                                    let response = execute(request);
                                    alpha.behaviour_mut().request_response
                                        .send_response(channel, response)
                                        .expect("alpha response channel is open");
                                }
                                request_response::Message::Response { response, .. } => {
                                    assert_eq!(response.id, "alpha-to-beta");
                                    assert_eq!(
                                        response.outcome,
                                        TaskOutcome::Success {
                                            result: TaskResult::Sum { value: 6.0 }
                                        }
                                    );
                                    return Ok::<(), anyhow::Error>(());
                                }
                            }
                        }
                    }
                    event = beta.select_next_some() => {
                        match event {
                            SwarmEvent::ConnectionEstablished { peer_id, .. }
                                if peer_id == alpha_id && !beta_request_sent =>
                            {
                                beta_request_sent = true;
                                beta.behaviour_mut().request_response.send_request(
                                    &alpha_id,
                                    TaskRequest {
                                        id: "beta-to-alpha".into(),
                                        task: Task::Echo { message: "hello alpha".into() },
                                    },
                                );
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(
                                request_response::Event::Message { message, .. }
                            )) => {
                                match message {
                                    request_response::Message::Request { request, channel, .. } => {
                                        let response = execute(request);
                                        beta.behaviour_mut().request_response
                                            .send_response(channel, response)
                                            .expect("beta response channel is open");
                                    }
                                    request_response::Message::Response { response, .. } => {
                                        assert_eq!(response.id, "beta-to-alpha");
                                        assert_eq!(
                                            response.outcome,
                                            TaskOutcome::Success {
                                                result: TaskResult::Echo {
                                                    message: "hello alpha".into()
                                                }
                                            }
                                        );

                                        if !alpha_request_sent {
                                            alpha_request_sent = true;
                                            alpha.behaviour_mut().request_response.send_request(
                                                &beta_id,
                                                TaskRequest {
                                                    id: "alpha-to-beta".into(),
                                                    task: Task::Sum { values: vec![1.0, 2.0, 3.0] },
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        })
        .await
        .context("two-node round trip timed out")??;

        Ok(())
    }
}
