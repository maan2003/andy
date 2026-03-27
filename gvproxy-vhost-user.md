# gvproxy + vhost-user Notes

This file captures the high-level decisions and working assumptions for a
possible `gvproxy`-backed networking path for `crosvm` without patching
`crosvm` itself.

This is not an implementation plan or protocol spec. It is a decision record
and a set of helpful notes for future work.

## Goal

- Provide VM networking for `crosvm` without depending on host TAP setup in the
  common case.
- Keep `crosvm` unchanged if possible.
- Prefer a separate backend process over invasive VMM changes.
- Make rootless or low-privilege setups more realistic than the normal
  TAP + bridge + NAT workflow.

## Chosen Direction

- Use a separate `vhost-user-net` backend process.
- Have `crosvm` connect to it using its existing `--vhost-user net,socket=...`
  support.
- Let that backend process talk to `gvproxy` internally.
- Keep the `gvproxy` integration behind the backend boundary so `crosvm` only
  sees a standard `virtio-net` / `vhost-user-net` device.

In short:

```text
guest virtio-net
  <-> crosvm vhost-user frontend
  <-> custom vhost-user-net backend
  <-> gvproxy
  <-> host network
```

## Why This Direction

- `crosvm` already supports `vhost-user` frontends, so we do not need to teach
  it about `gvproxy` directly.
- `gvproxy` does not speak `vhost-user`, so some adapter layer is required
  anyway.
- A separate backend process is a cleaner boundary than pushing more networking
  modes into `crosvm`'s Linux virtio-net path.
- This keeps the experiment modular: if the backend works, it may be reusable
  with other VMMs too.

## Explicit Non-Goals

- Do not modify `crosvm` unless a real incompatibility is found.
- Do not treat this as a peak-performance networking path.
- Do not try to preserve every TAP-specific offload feature from day one.
- Do not mix this with `vhost-net`; that is a different architecture.

## Key Architectural Decisions

### 1. `vhost-user` is the boundary

- `crosvm` should only know it is talking to a `net` backend.
- It should not know or care that the backend uses `gvproxy`.
- The backend is responsible for all proxy-specific packet transport logic.

### 2. `gvproxy` is behind the backend, not the frontend

- `gvproxy` is not a `vhost-user` device.
- The custom backend must translate between:
  - `virtio-net` queues exposed over `vhost-user`
  - whatever packet/socket interface `gvproxy` expects

### 3. Favor a standalone daemon

- The backend should be a normal standalone program.
- It should be testable independently from `crosvm`.
- It should be possible to swap the transport behind it later, for example:
  - `gvproxy`
  - `passt`
  - another userspace packet backend

### 4. Start simple

- Single queue pair is fine initially.
- Minimal virtio-net feature negotiation is fine initially.
- Correctness and debuggability matter more than squeezing out throughput.

## Why Not Patch `crosvm` First

- `crosvm`'s Linux net path is still TAP-oriented.
- The existing Linux `vhost-user-net` backend in `crosvm` is also TAP-shaped.
- Adding `gvproxy` natively inside `crosvm` is possible, but it couples the
  experiment to VMM internals earlier than necessary.
- A separate daemon is the smaller architectural commitment.

## Why Not Use `vhost-net`

- `vhost-net` accelerates the virtio-net <-> TAP path.
- It still assumes TAP-style host networking.
- It does not turn `gvproxy` into a kernel-accelerated backend.
- It does not solve the rootless / low-privilege networking problem we care
  about here.

## Performance Expectations

- This should be "good enough" networking, not the fastest possible path.
- Expect worse performance than TAP + `vhost-net`.
- Expect extra overhead from:
  - userspace packet handling
  - an extra process boundary
  - copies between guest buffers and proxy buffers
  - userspace NAT / socket forwarding logic

Reasonable expectation:

- Good for development, package installs, HTTP, SSH, test traffic, control
  plane traffic.
- Not the right choice for line-rate networking or latency-sensitive packet
  workloads.

## Why `gvproxy` Is Still Interesting

- It avoids normal host TAP plumbing in the common model.
- It is friendlier for rootless or restricted environments.
- It keeps host networking setup out of the user's main workflow.
- It fits the broader goal of "networking via a helper process" better than
  bridge/NAT shell recipes do.

## Practical Tips

### Keep the feature set narrow

- Do not advertise every virtio-net feature just because other backends do.
- Start from a conservative feature set and grow only when there is a reason.
- Multiqueue and fancy offloads can wait.

### Treat the backend as a translator

- The backend's job is not to be clever.
- It should mainly:
  - read guest virtio-net requests
  - turn them into proxy-facing packets
  - receive proxy-facing packets
  - place them back into guest queues correctly

### Optimize for debuggability

- Packet-path logging and queue-state logging will matter early.
- Clean disconnect handling will matter early.
- Failure modes should make it obvious whether the problem is:
  - `crosvm` frontend setup
  - `vhost-user` negotiation
  - guest virtio driver behavior
  - backend queue handling
  - `gvproxy` connectivity

### Do not overfit to `gvproxy`

- The first backend may target `gvproxy`.
- The daemon should still be shaped so a different packet transport can be
  dropped in later.
- A small transport abstraction is fine if it reflects a real split.

## Useful Reference Material

These are the most useful known references for this direction:

- `crosvm` existing `vhost-user` frontend support.
- `rust-vmm/vhost-user-backend` for a standalone Rust daemon model.
- `rust-vmm/vhost-device` template and existing backends for queue handling and
  daemon structure.
- `libkrun`'s `unixgram` / `unixstream` network proxy code for the transport
  side, even though it is not itself a `vhost-user` backend.
- DPDK `vhost` examples for a real userspace net backend reference, mainly for
  conceptual grounding rather than as a codebase to copy.

## Current Working Conclusion

- A separate `vhost-user-net` daemon is the cleanest first experiment.
- `crosvm` can likely remain unchanged.
- `gvproxy` should sit behind the backend, not inside `crosvm`.
- This is a convenience/modularity/rootless-oriented networking path, not a
  maximum-performance path.

## Open Questions

- What is the smallest correct virtio-net feature set for the first version?
- Should the first transport target be `gvproxy`, `passt`, or both?
- How much queue/control-plane behavior needs to be implemented for Linux guest
  drivers to behave predictably in practice?
- What level of metrics and packet tracing should be considered mandatory for
  the first backend prototype?
