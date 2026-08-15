use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use futures::StreamExt;
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, SwarmBuilder, identify, identity, mdns, ping,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
};
use swagri_core::{TASK_PROTOCOL_V1, Task, TaskRequest, TaskResponse};
use swagri_executor::execute;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "swagri-node",
    version,
    about = "Run an experimental Swagri peer"
)]
struct Args {
    /// Human-readable name shown in identify metadata and local output.
    #[arg(long, default_value = "swagri-node")]
    name: String,

    /// File containing the persistent Ed25519 node identity.
    #[arg(long, default_value = ".swagri/identity.key")]
    identity: PathBuf,

    /// QUIC multiaddress on which to accept peer connections.
    #[arg(long, default_value = "/ip4/0.0.0.0/udp/0/quic-v1")]
    listen: Multiaddr,

    /// Explicit peer address to dial. May be provided more than once.
    #[arg(long)]
    dial: Vec<Multiaddr>,

    /// Timeout applied to outbound task requests.
    #[arg(long, default_value_t = 30)]
    request_timeout_seconds: u64,
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "BehaviourEvent")]
struct Behaviour {
    mdns: mdns::tokio::Behaviour,
    request_response: request_response::cbor::Behaviour<TaskRequest, TaskResponse>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
}

#[derive(Debug)]
enum BehaviourEvent {
    Mdns(mdns::Event),
    RequestResponse(request_response::Event<TaskRequest, TaskResponse>),
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
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let keypair = load_or_create_identity(&args.identity)?;
    let local_peer_id = PeerId::from(keypair.public());
    let request_timeout = Duration::from_secs(args.request_timeout_seconds);

    let mut swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_behaviour(|key| {
            let local_peer_id = PeerId::from(key.public());
            let request_response = request_response::cbor::Behaviour::new(
                [(StreamProtocol::new(TASK_PROTOCOL_V1), ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(request_timeout),
            );

            Ok(Behaviour {
                mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), local_peer_id)?,
                request_response,
                identify: identify::Behaviour::new(
                    identify::Config::new("/swagri/identify/1".into(), key.public())
                        .with_agent_version(format!(
                            "swagri/{} ({})",
                            env!("CARGO_PKG_VERSION"),
                            args.name
                        )),
                ),
                ping: ping::Behaviour::default(),
            })
        })?
        .with_swarm_config(|config| config.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm
        .listen_on(args.listen.clone())
        .with_context(|| format!("could not listen on {}", args.listen))?;

    for address in args.dial {
        info!(%address, "dialing explicit peer address");
        swarm
            .dial(address.clone())
            .with_context(|| format!("could not dial {address}"))?;
    }

    let (completed_tx, mut completed_rx) = mpsc::unbounded_channel::<CompletedResponse>();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut known_peers = BTreeMap::<PeerId, BTreeSet<Multiaddr>>::new();
    let request_counter = AtomicU64::new(1);

    println!("Swagri node '{}'", args.name);
    println!("Peer ID: {local_peer_id}");
    println!("Identity: {}", args.identity.display());
    print_help();

    loop {
        tokio::select! {
            maybe_line = lines.next_line() => {
                match maybe_line.context("failed to read stdin")? {
                    Some(line) if !handle_command(
                        &line,
                        &mut swarm,
                        local_peer_id,
                        &request_counter,
                        &known_peers,
                    ) => break,
                    Some(_) => {}
                    None => break,
                }
            }
            event = swarm.select_next_some() => {
                handle_swarm_event(event, &mut swarm, &completed_tx, &mut known_peers);
            }
            Some(completed) = completed_rx.recv() => {
                if swarm
                    .behaviour_mut()
                    .request_response
                    .send_response(completed.channel, completed.response)
                    .is_err()
                {
                    warn!("requester disconnected before the response was sent");
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

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
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

fn handle_swarm_event(
    event: SwarmEvent<BehaviourEvent>,
    swarm: &mut libp2p::Swarm<Behaviour>,
    completed_tx: &mpsc::UnboundedSender<CompletedResponse>,
    known_peers: &mut BTreeMap<PeerId, BTreeSet<Multiaddr>>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            println!("Listening on {address}");
        }
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            known_peers.entry(peer_id).or_default();
            info!(peer = %peer_id, "peer connected");
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            info!(peer = %peer_id, ?cause, "peer disconnected");
        }
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
            for (peer_id, address) in peers {
                info!(peer = %peer_id, %address, "discovered peer through mDNS");
                known_peers
                    .entry(peer_id)
                    .or_default()
                    .insert(address.clone());
                swarm.add_peer_address(peer_id, address);
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
            handle_request_response(event, completed_tx);
        }
        SwarmEvent::Behaviour(BehaviourEvent::Identify(event)) => {
            debug!(?event, "identify event");
        }
        SwarmEvent::Behaviour(BehaviourEvent::Ping(event)) => {
            debug!(?event, "ping event");
        }
        _ => {}
    }
}

fn handle_request_response(
    event: request_response::Event<TaskRequest, TaskResponse>,
    completed_tx: &mpsc::UnboundedSender<CompletedResponse>,
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
            let completed_tx = completed_tx.clone();
            tokio::task::spawn_blocking(move || {
                let response = execute(request);
                let _ = completed_tx.send(CompletedResponse { channel, response });
            });
        }
        request_response::Event::Message {
            peer,
            message: request_response::Message::Response { response, .. },
            ..
        } => {
            println!(
                "Result from {peer}: id={} duration={}ms outcome={:?}",
                response.id, response.duration_ms, response.outcome
            );
        }
        request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        } => {
            warn!(peer = %peer, ?request_id, %error, "outbound task failed");
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

fn handle_command(
    line: &str,
    swarm: &mut libp2p::Swarm<Behaviour>,
    local_peer_id: PeerId,
    request_counter: &AtomicU64,
    known_peers: &BTreeMap<PeerId, BTreeSet<Multiaddr>>,
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
        "quit" | "exit" => return false,
        "echo" => {
            let result = parse_peer(&mut parts).and_then(|peer| {
                let message = parts.collect::<Vec<_>>().join(" ");
                if message.is_empty() {
                    bail!("echo requires a message");
                }
                Ok((peer, Task::Echo { message }))
            });
            submit_parsed(result, swarm, local_peer_id, request_counter);
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
            submit_parsed(result, swarm, local_peer_id, request_counter);
        }
        "sha256" => {
            let result = parse_peer(&mut parts).map(|peer| {
                let text = parts.collect::<Vec<_>>().join(" ");
                (peer, Task::Sha256 { text })
            });
            submit_parsed(result, swarm, local_peer_id, request_counter);
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
            submit_parsed(result, swarm, local_peer_id, request_counter);
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
) {
    match parsed {
        Ok((peer, task)) => {
            if let Err(error) = task.validate() {
                println!("Task rejected locally: {error}");
                return;
            }

            let sequence = request_counter.fetch_add(1, Ordering::Relaxed);
            let id = format!("{local_peer_id}-{sequence}");
            let task_kind = task.kind();
            swarm.behaviour_mut().request_response.send_request(
                &peer,
                TaskRequest {
                    id: id.clone(),
                    task,
                },
            );
            println!("Submitted {task_kind:?} task {id} to {peer}");
        }
        Err(error) => println!("Invalid command: {error:#}"),
    }
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
         echo <peer-id> <text>             Return text from the remote node\n\
         sum <peer-id> <numbers...>         Sum finite numbers remotely\n\
         sha256 <peer-id> <text>            Hash text remotely\n\
         benchmark <peer-id> <iterations>   Run bounded synthetic CPU work\n\
         quit                              Stop the node"
    );
}
