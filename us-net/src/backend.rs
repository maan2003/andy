use std::io::{self, Read, Write};
use std::mem::size_of;

use thiserror::Error;
use tracing::{debug, info, warn};
use vhost::vhost_user::message::{VhostUserProtocolFeatures, VhostUserVirtioFeatures};
use vhost_user_backend::{VhostUserBackendMut, VringRwLock, VringT};
use virtio_bindings::bindings::{
    virtio_config::VIRTIO_F_VERSION_1,
    virtio_net::VIRTIO_NET_F_MAC,
    virtio_ring::{VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC},
};
use virtio_queue::{DescriptorChain, QueueOwnedT, QueueT};
use vm_memory::{
    ByteValued, GuestAddressSpace, GuestMemoryAtomic, GuestMemoryLoadGuard, GuestMemoryMmap, Le16,
};
use vmm_sys_util::{
    epoll::EventSet,
    event::{EventConsumer, EventFlag, EventNotifier, new_event_consumer_and_notifier},
};

use crate::gvproxy::{GvproxyTransport, ReadError, WriteError};
use crate::mac::MacAddress;

const QUEUE_SIZE: usize = 256;
const NUM_QUEUES: usize = 2;
const RX_QUEUE_INDEX: usize = 0;
const TX_QUEUE_INDEX: usize = 1;
// Queue event ids are just the two guest kick slots owned by the worker.
// We drain both guest-facing queues on either kick, so their exact ordering
// does not leak into the backend logic.
const TX_QUEUE_EVENT: u16 = 0;
const RX_QUEUE_EVENT: u16 = 1;
const TRANSPORT_EVENT: u16 = NUM_QUEUES as u16 + 1;
const MAX_FRAME_SIZE: usize = 65_562;

type Result<T> = std::result::Result<T, BackendError>;
type NetDescriptorChain = DescriptorChain<GuestMemoryLoadGuard<GuestMemoryMmap<()>>>;

#[derive(Clone, Copy, Default)]
#[repr(C, packed)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    num_buffers: u16,
}

// SAFETY: This is a plain old data header matching the virtio-net wire layout.
unsafe impl ByteValued for VirtioNetHdr {}

const VNET_HDR_LEN: usize = size_of::<VirtioNetHdr>();

#[derive(Clone, Copy, Default)]
#[repr(C, packed)]
struct VirtioNetConfig {
    mac: [u8; 6],
    status: Le16,
    max_vq_pairs: Le16,
    mtu: Le16,
}

// SAFETY: This is a fixed-layout config struct with no padding-dependent references.
unsafe impl ByteValued for VirtioNetConfig {}

#[derive(Default)]
struct Stats {
    rx_packets: u64,
    tx_packets: u64,
    rx_drops: u64,
    tx_drops: u64,
    unsupported_tx_headers: u64,
}

struct GuestWriteResult {
    delivered: bool,
    used_any: bool,
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error("failed to create backend exit eventfd")]
    EventFd,
    #[error("device event set was not EPOLLIN-compatible: {0:?}")]
    BadEventSet(EventSet),
    #[error("unexpected device event {0}")]
    UnknownEvent(u16),
    #[error("guest memory is not available yet")]
    MissingMemory,
    #[error("failed to iterate queue descriptors")]
    DescriptorIterator,
    #[error("failed to write to guest memory")]
    GuestWrite,
    #[error("failed to read from guest memory")]
    GuestRead,
    #[error("failed to notify guest")]
    Notify,
    #[error("gvproxy read failed: {0}")]
    TransportRead(#[from] ReadError),
    #[error("gvproxy write failed: {0}")]
    TransportWrite(#[from] WriteError),
}

impl From<BackendError> for io::Error {
    fn from(err: BackendError) -> Self {
        io::Error::other(err)
    }
}

pub struct UsNetBackend {
    config: VirtioNetConfig,
    transport: GvproxyTransport,
    event_idx: bool,
    exit_consumer: EventConsumer,
    exit_notifier: EventNotifier,
    mem: Option<GuestMemoryLoadGuard<GuestMemoryMmap>>,
    pending_rx_len: usize,
    pending_tx_len: usize,
    rx_frame: [u8; MAX_FRAME_SIZE],
    tx_frame: [u8; MAX_FRAME_SIZE],
    stats: Stats,
}

impl UsNetBackend {
    pub fn new(mac: MacAddress, transport: GvproxyTransport) -> Result<Self> {
        let (exit_consumer, exit_notifier) = new_event_consumer_and_notifier(EventFlag::NONBLOCK)
            .map_err(|_| BackendError::EventFd)?;

        Ok(Self {
            config: VirtioNetConfig {
                mac: mac.0,
                status: 0.into(),
                max_vq_pairs: 1.into(),
                mtu: 0.into(),
            },
            transport,
            event_idx: false,
            exit_consumer,
            exit_notifier,
            mem: None,
            pending_rx_len: 0,
            pending_tx_len: 0,
            rx_frame: [0; MAX_FRAME_SIZE],
            tx_frame: [0; MAX_FRAME_SIZE],
            stats: Stats::default(),
        })
    }

    pub fn transport_fd(&self) -> i32 {
        self.transport.raw_fd()
    }

    pub fn transport_event_id() -> u64 {
        u64::from(TRANSPORT_EVENT)
    }

    fn device_ready(&self, vrings: &[VringRwLock]) -> bool {
        self.mem.is_some()
            && vrings.len() >= NUM_QUEUES
            && vrings[RX_QUEUE_INDEX].get_ref().get_queue().ready()
            && vrings[TX_QUEUE_INDEX].get_ref().get_queue().ready()
    }

    fn process_rx_queue(&mut self, vring: &VringRwLock) -> Result<()> {
        if self.mem.is_none() {
            return Ok(());
        }

        debug!("processing rx queue");
        self.flush_rx_path(vring)
    }

    fn process_transport_event(&mut self, evset: EventSet, vrings: &[VringRwLock]) -> Result<()> {
        if !self.device_ready(vrings) {
            return Ok(());
        }

        if evset.intersects(EventSet::HANG_UP | EventSet::READ_HANG_UP) {
            warn!("gvproxy transport reported hangup: {evset:?}");
        }

        if evset.contains(EventSet::IN) {
            self.flush_rx_path(&vrings[RX_QUEUE_INDEX])?;
        }

        if evset.contains(EventSet::OUT) && self.pending_tx_len > 0 {
            self.process_tx_queue(&vrings[TX_QUEUE_INDEX])?;
        }

        Ok(())
    }

    fn flush_rx_path(&mut self, vring: &VringRwLock) -> Result<()> {
        let mut used_any = false;

        if self.pending_rx_len > 0 {
            let frame = self.rx_frame[..self.pending_rx_len].to_vec();
            let result = self.write_frame_to_guest(vring, &frame)?;
            used_any |= result.used_any;
            if result.delivered {
                self.pending_rx_len = 0;
            } else {
                if used_any {
                    self.signal_if_needed(vring)?;
                }
                return Ok(());
            }
        }

        loop {
            self.rx_frame[..VNET_HDR_LEN].fill(0);

            let frame_len = match self
                .transport
                .read_frame(&mut self.rx_frame[VNET_HDR_LEN..])
            {
                Ok(len) => len,
                Err(ReadError::WouldBlock) => break,
                Err(err) => return Err(err.into()),
            };

            let total_len = VNET_HDR_LEN + frame_len;
            let frame = self.rx_frame[..total_len].to_vec();
            let result = self.write_frame_to_guest(vring, &frame)?;
            used_any |= result.used_any;

            if !result.delivered {
                self.pending_rx_len = total_len;
                debug!("rx queue is empty, deferring {frame_len}-byte frame");
                break;
            }
        }

        if used_any {
            self.signal_if_needed(vring)?;
        }

        Ok(())
    }

    fn write_frame_to_guest(
        &mut self,
        vring: &VringRwLock,
        frame: &[u8],
    ) -> Result<GuestWriteResult> {
        let mut used_any = false;

        for _ in 0..QUEUE_SIZE {
            let next = {
                let mut state = vring.get_mut();
                let mem = self
                    .mem
                    .as_ref()
                    .ok_or(BackendError::MissingMemory)?
                    .clone();
                let mut iter = state
                    .get_queue_mut()
                    .iter(mem.clone())
                    .map_err(|_| BackendError::DescriptorIterator)?;
                iter.next()
            };

            let Some(desc_chain) = next else {
                return Ok(GuestWriteResult {
                    delivered: false,
                    used_any,
                });
            };

            used_any = true;
            let head_index = desc_chain.head_index();
            let mut writer = desc_chain
                .clone()
                .writer(desc_chain.memory())
                .map_err(|_| BackendError::GuestWrite)?;

            if writer.available_bytes() < frame.len() {
                self.stats.rx_drops += 1;
                warn!(
                    "dropping rx frame because the guest did not provide a suitable buffer chain"
                );
                vring
                    .add_used(head_index, 0)
                    .map_err(|_| BackendError::Notify)?;
                continue;
            }

            writer
                .write_all(frame)
                .map_err(|_| BackendError::GuestWrite)?;
            self.stats.rx_packets += 1;
            debug!("delivered {} bytes from gvproxy to guest", frame.len());
            vring
                .add_used(head_index, writer.bytes_written() as u32)
                .map_err(|_| BackendError::Notify)?;
            return Ok(GuestWriteResult {
                delivered: true,
                used_any,
            });
        }

        Ok(GuestWriteResult {
            delivered: false,
            used_any,
        })
    }

    fn process_tx_queue(&mut self, vring: &VringRwLock) -> Result<()> {
        if self.mem.is_none() {
            return Ok(());
        }

        debug!("processing tx queue");
        let mut used_any = false;

        if self.resume_pending_tx()? {
            return Ok(());
        }

        loop {
            let next = {
                let mut state = vring.get_mut();
                let mem = self
                    .mem
                    .as_ref()
                    .ok_or(BackendError::MissingMemory)?
                    .clone();
                let mut iter = state
                    .get_queue_mut()
                    .iter(mem.clone())
                    .map_err(|_| BackendError::DescriptorIterator)?;
                iter.next()
            };

            let Some(desc_chain) = next else {
                debug!("tx queue had no available descriptors");
                break;
            };
            used_any = true;

            let head_index = desc_chain.head_index();
            let frame_len = match self.read_tx_frame(&desc_chain) {
                Ok(len) => len,
                Err(err) => {
                    self.stats.tx_drops += 1;
                    warn!("dropping tx frame: {err}");
                    vring
                        .add_used(head_index, 0)
                        .map_err(|_| BackendError::Notify)?;
                    continue;
                }
            };

            if frame_len <= VNET_HDR_LEN {
                self.stats.tx_drops += 1;
                warn!("dropping tx frame shorter than virtio-net header");
                vring
                    .add_used(head_index, 0)
                    .map_err(|_| BackendError::Notify)?;
                continue;
            }

            if !self.tx_frame[..VNET_HDR_LEN].iter().all(|byte| *byte == 0) {
                self.stats.tx_drops += 1;
                self.stats.unsupported_tx_headers += 1;
                warn!("dropping tx frame with unsupported virtio offload header");
                vring
                    .add_used(head_index, 0)
                    .map_err(|_| BackendError::Notify)?;
                continue;
            }

            match self
                .transport
                .write_frame(VNET_HDR_LEN, &mut self.tx_frame[..frame_len])
            {
                Ok(()) => {
                    self.stats.tx_packets += 1;
                    debug!(
                        "forwarded {} bytes from guest to gvproxy",
                        frame_len - VNET_HDR_LEN
                    );
                }
                Err(WriteError::WouldBlock | WriteError::PartialWrite) => {
                    self.pending_tx_len = frame_len;
                    debug!(
                        "deferring {}-byte guest frame until gvproxy becomes writable",
                        frame_len - VNET_HDR_LEN
                    );
                }
                Err(err) => return Err(err.into()),
            }

            vring
                .add_used(head_index, 0)
                .map_err(|_| BackendError::Notify)?;

            if self.pending_tx_len > 0 {
                break;
            }
        }

        if used_any {
            self.signal_if_needed(vring)?;
        }

        Ok(())
    }

    fn resume_pending_tx(&mut self) -> Result<bool> {
        if self.pending_tx_len == 0 {
            return Ok(false);
        }

        let result = if self.transport.has_unfinished_write() {
            self.transport
                .try_finish_write(VNET_HDR_LEN, &self.tx_frame[..self.pending_tx_len])
        } else {
            self.transport
                .write_frame(VNET_HDR_LEN, &mut self.tx_frame[..self.pending_tx_len])
        };

        match result {
            Ok(()) => {
                self.stats.tx_packets += 1;
                debug!(
                    "finished deferred {}-byte guest frame to gvproxy",
                    self.pending_tx_len - VNET_HDR_LEN
                );
                self.pending_tx_len = 0;
                Ok(false)
            }
            Err(WriteError::WouldBlock | WriteError::PartialWrite) => Ok(true),
            Err(err) => Err(err.into()),
        }
    }

    fn read_tx_frame(&mut self, desc_chain: &NetDescriptorChain) -> Result<usize> {
        let mut reader = desc_chain
            .clone()
            .reader(desc_chain.memory())
            .map_err(|_| BackendError::GuestRead)?;
        let frame_len = reader.available_bytes();
        if frame_len > self.tx_frame.len() {
            return Err(BackendError::GuestRead);
        }
        reader
            .read_exact(&mut self.tx_frame[..frame_len])
            .map_err(|_| BackendError::GuestRead)?;

        Ok(frame_len)
    }

    fn signal_if_needed(&mut self, vring: &VringRwLock) -> Result<()> {
        if vring
            .needs_notification()
            .map_err(|_| BackendError::Notify)?
        {
            vring
                .signal_used_queue()
                .map_err(|_| BackendError::Notify)?;
        }
        Ok(())
    }

    fn process_queue_event(&mut self, vring: &VringRwLock, kind: QueueKind) -> Result<()> {
        if matches!(kind, QueueKind::Tx) && self.event_idx {
            loop {
                vring
                    .disable_notification()
                    .map_err(|_| BackendError::Notify)?;
                self.process_queue_once(vring, kind)?;
                if !vring
                    .enable_notification()
                    .map_err(|_| BackendError::Notify)?
                {
                    break;
                }
            }
        } else {
            self.process_queue_once(vring, kind)?;
        }

        Ok(())
    }

    fn process_guest_queue_event(&mut self, vrings: &[VringRwLock]) -> Result<()> {
        // Under vhost-user the queue event id is the epoll registration slot,
        // not a transport-level contract. Drain both guest-facing queues on any
        // guest kick and let each path cheaply no-op when it has nothing to do.
        self.process_queue_event(&vrings[TX_QUEUE_INDEX], QueueKind::Tx)?;
        self.process_queue_event(&vrings[RX_QUEUE_INDEX], QueueKind::Rx)
    }

    fn process_queue_once(&mut self, vring: &VringRwLock, kind: QueueKind) -> Result<()> {
        match kind {
            QueueKind::Rx => self.process_rx_queue(vring),
            QueueKind::Tx => self.process_tx_queue(vring),
        }
    }
}

#[derive(Clone, Copy)]
enum QueueKind {
    Rx,
    Tx,
}

impl VhostUserBackendMut for UsNetBackend {
    type Vring = VringRwLock;
    type Bitmap = ();

    fn num_queues(&self) -> usize {
        NUM_QUEUES
    }

    fn max_queue_size(&self) -> usize {
        QUEUE_SIZE
    }

    fn features(&self) -> u64 {
        (1 << VIRTIO_F_VERSION_1)
            | (1 << VIRTIO_NET_F_MAC)
            | (1 << VIRTIO_RING_F_INDIRECT_DESC)
            | (1 << VIRTIO_RING_F_EVENT_IDX)
            | VhostUserVirtioFeatures::PROTOCOL_FEATURES.bits()
    }

    fn protocol_features(&self) -> VhostUserProtocolFeatures {
        // Without MQ, crosvm falls back to the spec minimum for net devices
        // and assumes a third control queue that this backend does not implement.
        VhostUserProtocolFeatures::CONFIG | VhostUserProtocolFeatures::MQ
    }

    fn reset_device(&mut self) {
        self.pending_rx_len = 0;
        self.pending_tx_len = 0;
        self.event_idx = false;
        info!(
            "resetting us-net backend; stats: rx_packets={}, tx_packets={}, rx_drops={}, tx_drops={}, unsupported_tx_headers={}",
            self.stats.rx_packets,
            self.stats.tx_packets,
            self.stats.rx_drops,
            self.stats.tx_drops,
            self.stats.unsupported_tx_headers,
        );
    }

    fn set_event_idx(&mut self, enabled: bool) {
        self.event_idx = enabled;
    }

    fn get_config(&self, offset: u32, size: u32) -> Vec<u8> {
        let config = self.config.as_slice();
        let start = usize::min(offset as usize, config.len());
        let end = usize::min(start.saturating_add(size as usize), config.len());
        config[start..end].to_vec()
    }

    fn update_memory(&mut self, mem: GuestMemoryAtomic<GuestMemoryMmap>) -> io::Result<()> {
        self.mem = Some(mem.memory());
        Ok(())
    }

    fn handle_event(
        &mut self,
        device_event: u16,
        evset: EventSet,
        vrings: &[VringRwLock],
        _thread_id: usize,
    ) -> io::Result<()> {
        if device_event != TRANSPORT_EVENT && !evset.contains(EventSet::IN) {
            return Err(BackendError::BadEventSet(evset).into());
        }

        debug!("handle_event device_event={device_event} evset={evset:?}");
        match device_event {
            RX_QUEUE_EVENT | TX_QUEUE_EVENT => self.process_guest_queue_event(vrings)?,
            TRANSPORT_EVENT => self.process_transport_event(evset, vrings)?,
            other => return Err(BackendError::UnknownEvent(other).into()),
        }

        Ok(())
    }

    fn exit_event(&self, _thread_index: usize) -> Option<(EventConsumer, EventNotifier)> {
        Some((
            self.exit_consumer.try_clone().ok()?,
            self.exit_notifier.try_clone().ok()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd, OwnedFd};

    use nix::sys::socket::{AddressFamily, MsgFlags, SockFlag, SockType, recv, send, socketpair};
    use virtio_bindings::bindings::virtio_ring::VRING_DESC_F_WRITE;
    use virtio_queue::{desc::split::Descriptor as SplitDescriptor, mock::MockSplitQueue};
    use vm_memory::{Address, Bytes, GuestAddress, GuestMemoryAtomic, GuestMemoryMmap};

    use super::*;

    const QUEUE_LEN: u16 = 16;
    const RX_QUEUE_START: GuestAddress = GuestAddress(0x1_000);
    const TX_QUEUE_START: GuestAddress = GuestAddress(0x4_000);
    const RX_BUFFER_ADDR: GuestAddress = GuestAddress(0x8_000);
    const TX_BUFFER_ADDR: GuestAddress = GuestAddress(0x9_000);

    fn new_backend() -> (UsNetBackend, OwnedFd) {
        let (backend_fd, peer_fd) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .unwrap();

        (
            UsNetBackend::new(
                MacAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]),
                GvproxyTransport::from_fd(backend_fd),
            )
            .unwrap(),
            peer_fd,
        )
    }

    fn new_guest_memory() -> (GuestMemoryAtomic<GuestMemoryMmap>, GuestMemoryMmap) {
        let raw_mem = GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x20_000)]).unwrap();
        (GuestMemoryAtomic::new(raw_mem.clone()), raw_mem)
    }

    fn new_vring(
        mem: &GuestMemoryAtomic<GuestMemoryMmap>,
        queue: &MockSplitQueue<GuestMemoryMmap>,
    ) -> VringRwLock {
        let vring = VringRwLock::new(mem.clone(), QUEUE_LEN).unwrap();
        vring.set_queue_size(QUEUE_LEN);
        vring
            .set_queue_info(
                queue.desc_table_addr().0,
                queue.avail_addr().0,
                queue.used_addr().0,
            )
            .unwrap();
        vring.set_queue_ready(true);
        vring
    }

    fn make_tx_descriptor(addr: GuestAddress, len: usize) -> virtio_queue::desc::RawDescriptor {
        SplitDescriptor::new(addr.0, len as u32, 0, 0).into()
    }

    fn make_rx_descriptor(addr: GuestAddress, len: usize) -> virtio_queue::desc::RawDescriptor {
        SplitDescriptor::new(addr.0, len as u32, VRING_DESC_F_WRITE as u16, 0).into()
    }

    fn publish_descriptor(
        mem: &GuestMemoryMmap,
        queue: &MockSplitQueue<GuestMemoryMmap>,
        descriptor: virtio_queue::desc::RawDescriptor,
    ) {
        queue.desc_table().store(0, descriptor).unwrap();
        mem.write_obj(0u16, queue.avail_addr().unchecked_add(4))
            .unwrap();
        mem.write_obj(1u16, queue.avail_addr().unchecked_add(2))
            .unwrap();
    }

    fn send_all(fd: &OwnedFd, buf: &[u8]) {
        let mut written = 0usize;
        while written < buf.len() {
            written += send(fd.as_raw_fd(), &buf[written..], MsgFlags::MSG_NOSIGNAL).unwrap();
        }
    }

    fn recv_exact(fd: &OwnedFd, buf: &mut [u8]) {
        let mut read = 0usize;
        while read < buf.len() {
            let size = recv(fd.as_raw_fd(), &mut buf[read..], MsgFlags::MSG_WAITALL).unwrap();
            assert!(size > 0);
            read += size;
        }
    }

    fn send_qemu_frame(fd: &OwnedFd, payload: &[u8]) {
        send_all(fd, &(payload.len() as u32).to_be_bytes());
        send_all(fd, payload);
    }

    #[test]
    fn tx_queue_event_uses_queue_one_without_polling() {
        let (mut backend, peer_fd) = new_backend();
        let (mem, raw_mem) = new_guest_memory();
        let rx_queue = MockSplitQueue::create(&raw_mem, RX_QUEUE_START, QUEUE_LEN);
        let tx_queue = MockSplitQueue::create(&raw_mem, TX_QUEUE_START, QUEUE_LEN);
        let vrings = vec![new_vring(&mem, &rx_queue), new_vring(&mem, &tx_queue)];

        let payload = [0xde, 0xad, 0xbe, 0xef];
        let mut guest_frame = vec![0u8; VNET_HDR_LEN];
        guest_frame.extend_from_slice(&payload);
        raw_mem.write_slice(&guest_frame, TX_BUFFER_ADDR).unwrap();
        publish_descriptor(
            &raw_mem,
            &tx_queue,
            make_tx_descriptor(TX_BUFFER_ADDR, guest_frame.len()),
        );

        backend.update_memory(mem).unwrap();
        backend
            .handle_event(TX_QUEUE_EVENT, EventSet::IN, &vrings, 0)
            .unwrap();

        let mut framed = vec![0u8; 4 + payload.len()];
        recv_exact(&peer_fd, &mut framed);
        assert_eq!(&framed[..4], &(payload.len() as u32).to_be_bytes());
        assert_eq!(&framed[4..], &payload);
        assert_eq!(tx_queue.used().idx().load(), 1);
    }

    #[test]
    fn rx_queue_event_flushes_pending_frame_without_timer() {
        let (mut backend, peer_fd) = new_backend();
        let (mem, raw_mem) = new_guest_memory();
        let rx_queue = MockSplitQueue::create(&raw_mem, RX_QUEUE_START, QUEUE_LEN);
        let tx_queue = MockSplitQueue::create(&raw_mem, TX_QUEUE_START, QUEUE_LEN);
        let vrings = vec![new_vring(&mem, &rx_queue), new_vring(&mem, &tx_queue)];

        let payload = [0xca, 0xfe, 0xba, 0xbe];
        send_qemu_frame(&peer_fd, &payload);

        backend.update_memory(mem).unwrap();
        backend
            .handle_event(TRANSPORT_EVENT, EventSet::IN, &vrings, 0)
            .unwrap();
        assert_eq!(backend.pending_rx_len, VNET_HDR_LEN + payload.len());
        assert_eq!(rx_queue.used().idx().load(), 0);

        publish_descriptor(
            &raw_mem,
            &rx_queue,
            make_rx_descriptor(RX_BUFFER_ADDR, VNET_HDR_LEN + payload.len()),
        );

        backend
            .handle_event(RX_QUEUE_EVENT, EventSet::IN, &vrings, 0)
            .unwrap();

        let mut guest_frame = vec![0u8; VNET_HDR_LEN + payload.len()];
        raw_mem
            .read_slice(&mut guest_frame, RX_BUFFER_ADDR)
            .unwrap();
        assert_eq!(&guest_frame[..VNET_HDR_LEN], &[0u8; VNET_HDR_LEN]);
        assert_eq!(&guest_frame[VNET_HDR_LEN..], &payload);
        assert_eq!(backend.pending_rx_len, 0);
        assert_eq!(rx_queue.used().idx().load(), 1);
    }
}
