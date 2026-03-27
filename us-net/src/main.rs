mod backend;
mod gvproxy;
mod mac;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use argh::FromArgs;
use backend::UsNetBackend;
use gvproxy::GvproxyTransport;
use mac::MacAddress;
use tracing::info;
use tracing_subscriber::EnvFilter;
use vhost_user_backend::VhostUserDaemon;
use vm_memory::{GuestMemoryAtomic, GuestMemoryMmap};
use vmm_sys_util::epoll::EventSet;

#[derive(FromArgs)]
/// Serve a narrow vhost-user-net backend backed by gvproxy.
struct Args {
    #[argh(option)]
    /// unix socket path for the vhost-user frontend connection
    socket: PathBuf,

    #[argh(option)]
    /// unix stream socket path exposed by gvproxy --listen-qemu
    gvproxy: PathBuf,

    #[argh(option)]
    /// guest-visible MAC address (defaults to a generated locally-administered address)
    mac: Option<MacAddress>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args: Args = argh::from_env();
    let mac = args.mac.unwrap_or_else(MacAddress::generated).validate()?;
    let transport = GvproxyTransport::open(&args.gvproxy).with_context(|| {
        format!(
            "failed to connect to gvproxy socket {}",
            args.gvproxy.display()
        )
    })?;

    let backend = Arc::new(Mutex::new(UsNetBackend::new(mac, transport)?));
    let transport_fd = backend.lock().unwrap().transport_fd();

    let memory = GuestMemoryAtomic::new(GuestMemoryMmap::new());
    let mut daemon = VhostUserDaemon::new("us-net".to_string(), backend.clone(), memory)
        .map_err(|err| anyhow!("failed to create vhost-user daemon: {err}"))?;

    let handlers = daemon.get_epoll_handlers();
    if handlers.len() != 1 {
        bail!("expected exactly one epoll handler, got {}", handlers.len());
    }

    handlers[0]
        .register_listener(
            transport_fd,
            EventSet::IN
                | EventSet::OUT
                | EventSet::EDGE_TRIGGERED
                | EventSet::READ_HANG_UP,
            UsNetBackend::transport_event_id(),
        )
        .context("failed to register gvproxy socket with epoll handler")?;

    info!(
        "starting us-net on {} -> gvproxy {} (qemu stream, mac {})",
        args.socket.display(),
        args.gvproxy.display(),
        mac,
    );

    daemon.serve(&args.socket).map_err(|err| {
        anyhow!(
            "vhost-user daemon failed while serving {}: {err}",
            args.socket.display()
        )
    })
}
