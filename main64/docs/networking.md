# KAOS Rust Kernel: The Networking Stack (`lib_net`)

> Audience: readers with no previous experience in network protocol implementation. The document assumes only the general driver background from [`docs/drivers.md`](drivers.md) — namely that a NIC driver is an ordinary Ring-3 process that owns a PCI card through MMIO and DMA.

This document explains how KAOS turns a stream of raw bytes coming out of a network card into a working `ping`, and back again. It covers exactly one crate, [`lib_net`](../lib_net/src/lib.rs), which is a small, hardware-agnostic, `#![no_std]` implementation of Ethernet II, ARP, IPv4, and ICMP Echo. Everything in this crate operates on byte slices; it never touches a PCI register, an MMIO address, or a DMA buffer. That separation is the whole point of its design, and this document follows the same layering the code itself uses, mapped onto the classic OSI reference model.

Every wire format below is documented **byte by byte**: for each protocol there is first an ASCII diagram of the layout, then a table listing every single field's exact offset, length, meaning, and the concrete value KAOS reads or writes there. Nothing is left as "the rest of the header" — if a byte exists on the wire, it has a row in one of these tables.

The relevant files are:

* [`lib_net/src/nic.rs`](../lib_net/src/nic.rs) — the hardware boundary (`NicDevice` trait)
* [`lib_net/src/proto/ethernet.rs`](../lib_net/src/proto/ethernet.rs) — OSI Layer 2, Ethernet II framing
* [`lib_net/src/proto/arp.rs`](../lib_net/src/proto/arp.rs) — address resolution and the dynamic ARP cache
* [`lib_net/src/proto/ipv4.rs`](../lib_net/src/proto/ipv4.rs) — OSI Layer 3, IPv4 headers and checksums
* [`lib_net/src/proto/icmp.rs`](../lib_net/src/proto/icmp.rs) — ICMP Echo Request/Reply (`ping`)
* [`lib_net/src/stack.rs`](../lib_net/src/stack.rs) — the coordinator that wires the layers together
* [`lib_net/src/config.rs`](../lib_net/src/config.rs), [`lib_net/src/event.rs`](../lib_net/src/event.rs) — supporting types
* [`lib_driver_runtime/src/repl.rs`](../lib_driver_runtime/src/repl.rs) — the background driver loop that owns a live `NetworkStack`
* [`user_programs/net_tools/src/main.rs`](../user_programs/net_tools/src/main.rs) — the `ping`/`arp`/`ifconfig` client program

---

## 1. Where `lib_net` Sits in the OSI Model

The OSI model describes seven layers, but only four of them have a concrete counterpart in this codebase:

```text
OSI Layer                    KAOS implementation
---------------------------  --------------------------------------------
7  Application               net-tools.bin (ping/arp/ifconfig REPL)
4  Transport                 (not implemented — no TCP, no UDP)
3  Network                   lib_net::proto::ipv4  +  lib_net::proto::icmp
2½ (address resolution)      lib_net::proto::arp
2  Data Link                 lib_net::proto::ethernet
1  Physical                  drivers/rtl8139, drivers/intel_nic (PCI/MMIO/DMA)
```

ARP does not have a clean OSI layer of its own. It carries IPv4 addresses (a Layer 3 concept) inside an Ethernet frame (a Layer 2 concept) in order to answer a Layer-2 question — "which MAC address should I put in the destination field?" Networking textbooks usually place it at the boundary between Layer 2 and Layer 3, and this document keeps it there, directly after Ethernet.

ICMP is, strictly speaking, encapsulated *inside* IPv4 (its packets carry protocol number 1), not a peer of IPv4. It is not a transport protocol like TCP or UDP either — no port numbers, no connections. This document treats it as part of Layer 3, immediately after the IPv4 section, because that mirrors both the OSI convention and the code's own module layout (`icmp.rs` calls back into `ipv4::compute_checksum`).

There is no Layer 4 in this codebase at all: no TCP, no UDP. `ping` works because ICMP Echo does not need a transport protocol underneath it — it rides directly on IPv4. This is why `lib_net` can offer a working `ping` while still being a genuinely minimal stack.

Every layer below is implemented as a pair of pure functions, `parse()` and `serialize()`, operating on `&[u8]` slices, plus a handful of small value types (`MacAddress`, `Ipv4Address`). None of them allocate on the receive path — a parsed frame only ever *borrows* into the original byte buffer.

---

## 2. The Hardware Boundary: `NicDevice`

Before looking at any protocol, it helps to see the shape of the hole `lib_net` was built to fit into. [`nic.rs`](../lib_net/src/nic.rs) defines a four-method trait:

```rust
pub trait NicDevice {
    fn mac(&self) -> MacAddress;
    fn transmit(&mut self, packet: &[u8]) -> Result<(), SysError>;
    fn poll_next_packet(&mut self, out_buf: &mut [u8]) -> Option<usize>;
    fn shutdown(&mut self);
}
```

Both concrete NIC drivers — `Rtl8139Device` in [`drivers/rtl8139/src/rtl8139.rs`](../drivers/rtl8139/src/rtl8139.rs) and the Intel NIC driver in [`drivers/intel_nic/src/intel_nic.rs`](../drivers/intel_nic/src/intel_nic.rs) — implement exactly this trait. Everything about PCI enumeration, MMIO register offsets, DMA ring buffers, and the RTL8139's four-descriptor transmit ring belongs to those crates and to `docs/drivers.md`; `lib_net` never sees any of it. It only sees `transmit(&[u8])` and `poll_next_packet() -> Option<usize>`.

This is the reason the protocol code can be unit-tested on a normal host machine (`cargo test -p lib_net`) without any emulator: nothing in this crate depends on `x86_64-unknown-none`, port I/O, or QEMU. A test simply calls `parse()` on a byte array it wrote by hand.

---

## 3. Layer 2: Ethernet II Framing

An Ethernet II frame is the outermost envelope every other layer lives inside. Its header is exactly 14 bytes:

```text
byte offset   0            6            12      14
              +------------+------------+-------+-----------------+
              | Destination| Source MAC | Type  | Payload         |
              | MAC 6 bytes| 6 bytes    | 2 B   | (variable)      |
              +------------+------------+-------+-----------------+
```

| Byte offset | Length | Field | Meaning |
|---|---|---|---|
| 0–5 | 6 bytes | Destination MAC | The 48-bit hardware address this frame is delivered to: either one specific NIC's address, or the all-`0xFF` broadcast address for ARP requests and any other segment-wide traffic. |
| 6–11 | 6 bytes | Source MAC | The 48-bit hardware address of the NIC that transmitted this frame — for anything KAOS sends, always the local hardware's own burned-in address. |
| 12–13 | 2 bytes | EtherType | A big-endian `u16` identifying the protocol carried in the payload. `0x0800` = IPv4, `0x0806` = ARP. Any other value is parsed successfully (the header itself is still valid) but then dropped unhandled by `NetworkStack` (§7). |
| 14–end | variable | Payload | The Layer-3 (or ARP) packet this frame carries, `total_frame_length − 14` bytes long. `lib_net` never copies this out; it hands back a borrowed slice `&data[14..]`. |

[`ethernet.rs`](../lib_net/src/proto/ethernet.rs) models this as:

```rust
pub struct EthernetFrame<'a> {
    pub dest_mac: MacAddress,
    pub src_mac: MacAddress,
    pub ethertype: u16,
    pub payload: &'a [u8],
}
```

`MacAddress` is a thin wrapper around `[u8; 6]`. It defines `BROADCAST` (`FF:FF:FF:FF:FF:FF`), `ZERO`, and two predicates worth knowing at the byte level: `is_broadcast()` compares against all-`0xFF`, and `is_multicast()` tests bit 0 of the *first* transmitted octet (`self.0[0] & 0x01`) — the IEEE 802.3 convention that the least-significant bit of the first byte on the wire marks a frame as multicast/broadcast rather than a specific unicast station.

`EthernetFrame::parse()` rejects any slice shorter than 14 bytes (a "runt frame"), then does no more than three fixed-offset slice copies and one big-endian `u16` read for the EtherType:

```rust
let ethertype = u16::from_be_bytes([data[12], data[13]]);
let payload = &data[ETHERNET_HEADER_LEN..];
```

Network protocols always encode multi-byte integers in **big-endian** ("network byte order"), regardless of the CPU's native endianness. x86_64 is little-endian, so every field wider than one byte in this crate is deliberately read with `from_be_bytes` and written with `to_be_bytes` — never the native `from_ne_bytes`. This single detail is the most common source of an "it silently does the wrong thing" bug in a hand-rolled network stack, and it recurs in every layer below.

The two currently recognized EtherTypes are:

```rust
pub mod ethertype {
    pub const IPV4: u16 = 0x0800;
    pub const ARP:  u16 = 0x0806;
}
```

`payload` is `&data[14..]` — a **slice**, not a copy. Parsing an Ethernet frame costs no allocation and no `memcpy`; the returned `EthernetFrame<'a>` simply borrows a sub-range of whatever buffer the caller already owns (typically the driver's DMA receive ring, copied once into a stack-local array by the driver). Every other `parse()` function in this crate follows the same zero-copy convention, which is why `lib_net` needs no heap activity at all on the receive path.

`serialize()` is the mirror image: it writes destination MAC, source MAC, big-endian EtherType, and then the payload, into a caller-supplied `buf`, and fails (`None`) if `buf` is too small for `14 + payload.len()` bytes.

### 3.1 The 60-Byte Minimum Frame and Where Padding Comes From

Ethernet defines 60 bytes (excluding the 4-byte hardware-generated Frame Check Sequence, which never appears in software at all — it is computed and appended by the NIC's transmit logic and stripped by its receive logic before `lib_net` ever sees a byte) as the smallest legal frame. `ethernet.rs` declares this as `MIN_ETHERNET_FRAME_LEN = 60`, but the constant itself is documentation only — the actual enforcement happens one level up, in `stack.rs`, using a small trick worth understanding precisely:

```rust
let mut out_frame = [0u8; 60]; // zero-initialized!
if let Some(frame_len) = eth_reply.serialize(&mut out_frame) {
    let final_len = frame_len.max(60);
    tx_fn(&out_frame[..final_len]);
}
```

An ARP reply's real length is `14 + 28 = 42` bytes. `serialize()` writes exactly those 42 bytes and returns `Some(42)`. But `out_frame` was declared as `[0u8; 60]` — Rust guarantees a stack array literal like this is fully zeroed before any of it is written — so bytes `42..60` are already zero. Taking `frame_len.max(60)` and slicing `&out_frame[..60]` therefore transmits 42 real bytes followed by 18 zero-padding bytes, without any explicit padding loop. The same pattern appears for ICMP replies (`[0u8; 1536]`) and for the two "build" helpers (`build_arp_request`, `build_ping`), always ending in `.max(60)`.

This matters for a newcomer to notice because the *driver* layer underneath repeats the same `.max(60)` clamp on the transmit path (see `drivers/rtl8139/src/rtl8139.rs`), but without pre-zeroing its DMA slot — meaning it trusts whatever `lib_net` handed it to already be padded correctly. `lib_net` upholds that contract by construction; a caller that assembles a raw Ethernet frame *outside* `lib_net`'s helpers and hands a slice shorter than 60 bytes directly to a driver would not get this guarantee. See `docs/drivers.md` §15 for that hardware-side caveat.

---

## 4. Address Resolution: ARP

### 4.1 Why ARP Exists

Applications and IP routing reason exclusively in terms of IPv4 addresses. Ethernet hardware, however, delivers frames purely by MAC address — it has no concept of an IP address at all. Before KAOS can send a single IPv4 packet to `192.168.1.1`, it therefore has to answer a purely Layer-2 question: *which* MAC address currently owns that IP on this LAN segment? The Address Resolution Protocol, defined in RFC 826, answers exactly that question by broadcasting "Who has `192.168.1.1`? Tell `192.168.1.200`" and waiting for the owner to reply "`192.168.1.1` is at `52:54:00:12:34:56`".

### 4.2 Byte Layout

An Ethernet/IPv4 ARP packet is a fixed 28 bytes, carried as the *payload* of an Ethernet frame whose EtherType is `0x0806`:

```text
byte offset  0        2        4    5    6        8            14           18           24        28
             +--------+--------+----+----+--------+------------+------------+------------+---------+
             | HWtype | Ptype  | HL | PL | Opcode | Sender MAC | Sender IP  | Target MAC | Target IP|
             | 2 B    | 2 B    | 1B | 1B | 2 B    | 6 bytes    | 4 bytes    | 6 bytes    | 4 bytes  |
             +--------+--------+----+----+--------+------------+------------+------------+---------+
```

| Byte offset | Length | Field (RFC 826 name) | Meaning | Value KAOS reads or writes |
|---|---|---|---|---|
| 0–1 | 2 bytes | Hardware Type (HTYPE) | Identifies the link-layer technology the hardware addresses below belong to, so a generic ARP implementation could size and interpret them correctly for any link layer. | Always `1` (Ethernet). `ArpPacket::parse()` returns `None` for any other value — this stack only ever speaks ARP over Ethernet. |
| 2–3 | 2 bytes | Protocol Type (PTYPE) | Identifies the network-layer protocol whose address is being resolved, using the same 16-bit numbering as Ethernet's own EtherType field. | Always `0x0800` (IPv4). `parse()` returns `None` for any other value — this stack only ever resolves IPv4 addresses. |
| 4 | 1 byte | Hardware Address Length (HLEN) | The byte length of one hardware address, letting a generic parser size the SHA/THA fields below without needing a lookup table keyed on HTYPE. | Always `6` (an Ethernet MAC address is 6 bytes). `parse()` returns `None` for any other value. |
| 5 | 1 byte | Protocol Address Length (PLEN) | The byte length of one protocol address, sizing the SPA/TPA fields below. | Always `4` (an IPv4 address is 4 bytes). `parse()` returns `None` for any other value. |
| 6–7 | 2 bytes | Operation (opcode) | What this packet is asking or announcing. | `1` = Request ("who has TPA? tell SHA/SPA"), `2` = Reply ("SPA is at SHA"). Any other value produces `NetworkEvent::None` further up in `stack.rs` (§4.4). |
| 8–13 | 6 bytes | Sender Hardware Address (SHA) | The MAC address of the host that generated this packet. | On a Request, the local host's own MAC. On a Reply, the MAC address being announced as the answer. |
| 14–17 | 4 bytes | Sender Protocol Address (SPA) | The IPv4 address of the host that generated this packet. | On a Request, the local host's own configured IP (`NetworkConfig::ip`). On a Reply, the IPv4 address being announced. |
| 18–23 | 6 bytes | Target Hardware Address (THA) | The MAC address of the addressee, if it is already known. | All-zero (`MacAddress::ZERO`) on a Request — this is precisely the field a Request exists to fill in, so the requester cannot know it yet. Filled in with the original requester's own MAC on a Reply. |
| 24–27 | 4 bytes | Target Protocol Address (TPA) | The IPv4 address this packet is asking about (Request) or confirming (Reply). | The address KAOS wants resolved, on a Request it sends. The original requester's own IP, echoed back, on a Reply KAOS sends. |

Every one of these eight fields is a plain, fixed-width, big-endian value or byte array at a fixed offset — there are no ARP options and no variable-length extensions in this implementation, matching the real RFC 826 wire format for Ethernet-over-IPv4 exactly.

[`arp.rs`](../lib_net/src/proto/arp.rs) validates the four fixed identity fields (HTYPE, PTYPE, HLEN, PLEN) on every parse rather than trusting the sender, rejecting any packet — malformed or, in principle, maliciously crafted — that claims to be for a different link layer or network layer:

```rust
if hardware_type != HARDWARE_TYPE_ETHERNET
    || protocol_type != PROTOCOL_TYPE_IPV4
    || hardware_len != 6
    || protocol_len != 4
{
    return None;
}
```

Opcode `1` means Request, opcode `2` means Reply:

```rust
pub mod opcode {
    pub const REQUEST: u16 = 1;
    pub const REPLY: u16 = 2;
}
```

`ArpPacket::build_request()` fills SHA/SPA with the local host's own values and TPA with the address being resolved, and — because THA is, by definition, not yet known — sets it to `MacAddress::ZERO`. The surrounding `EthernetFrame` is then sent to `MacAddress::BROADCAST`, since there is no unicast destination to address it to yet. `build_reply()` is the same 28-byte shape with the roles reversed: SHA/SPA become the local host's own identity, and THA/TPA are filled in with the original requester's SHA/SPA.

### 4.3 The Dynamic ARP Cache (`ArpTable`)

Resolving an address by broadcast on every single packet would be wasteful and slow, so KAOS caches the answer:

```rust
pub struct ArpTable {
    entries: Vec<(Ipv4Address, MacAddress)>,
}
```

This is deliberately the simplest possible data structure: an unsorted `Vec` of `(IP, MAC)` pairs, searched linearly in `lookup()`. There is no hash map, because the table is capped at `MAX_ENTRIES = 128` and a linear scan over at most 128 short tuples is not a real performance concern inside a single-segment LAN driver.

`update()` first scans for an existing entry for that IP and overwrites its MAC in place (a host's MAC can legitimately change, e.g. after a NIC replacement). Only if the IP is genuinely new *and* the table is already full does it evict — always the **oldest** entry, at index `0`, via `Vec::remove(0)`:

```rust
if self.entries.len() >= MAX_ENTRIES {
    self.entries.remove(0);
}
self.entries.push((ip, mac));
```

This bound exists specifically as a defensive measure: without it, a flood of ARP packets carrying distinct forged sender IPs (an attacker does not even need a reply — see below) would grow this `Vec` without limit and exhaust the owning process's heap. `MAX_ENTRIES = 128` is a small, arbitrary but sufficient limit for a machine's own LAN segment.

Two details are worth flagging explicitly for a newcomer, because they differ from a production TCP/IP stack:

- **There is no expiration timer.** A real operating system ages ARP entries out after some number of minutes, because a host's MAC-to-IP binding can change (DHCP lease change, NIC swap) without that host ever sending an unsolicited announcement. `ArpTable` entries live forever until either overwritten by a fresher packet for the same IP, or evicted purely because the table is full. In a QEMU lab environment with a small number of static hosts this is harmless; on a real, changing network it would eventually go stale.
- **Every observed ARP packet updates the cache — including ones the local host never asked about.** `process_arp()` in `stack.rs` calls `self.arp_table.update(arp.sender_ip, arp.sender_mac)` unconditionally, before even looking at the opcode. This means the cache learns passively from *any* ARP traffic on the segment, not only from replies to requests this host issued (this is close to, but not identical to, what RFC 826 calls "gratuitous" learning). On a trusted lab network this is a convenient simplification; on a shared or adversarial network it means any host can plant an entry in this cache just by sending one ARP packet with a forged sender IP, without the target IP ever needing to be reachable — this is precisely the ARP cache poisoning technique, and `lib_net` implements no defense against it. That absence is an accepted, documented limitation of this educational stack, not an oversight to silently work around.

### 4.4 How the Stack Answers and Learns

`NetworkStack::process_arp()` (in `stack.rs`) is where the cache and the wire protocol meet:

1. Parse the 28-byte payload; bail out silently (`NetworkEvent::None`) if it fails validation.
2. Unconditionally cache `(sender_ip, sender_mac)` — see above.
3. If `opcode == REQUEST` and `target_ip` matches this host's own configured IP, build and transmit an ARP Reply, and report `NetworkEvent::ArpRequestAnswered` to the caller.
4. If `opcode == REPLY`, report `NetworkEvent::ArpReplyReceived` — the cache was already updated in step 2, so this event exists purely so a caller such as `net-tools` can notice "the address I was waiting for just resolved" without polling the table in a busy loop.
5. Any other opcode, or a request for an IP that is not this host's own, produces `NetworkEvent::None`.

---

## 5. Layer 3: IPv4

### 5.1 Byte Layout

[`ipv4.rs`](../lib_net/src/proto/ipv4.rs) only ever produces, and only fully validates, the plain 20-byte header (IHL = 5, no IP options):

```text
byte offset   0        1        2            4            6         8     9         10           12          16          20
              +--------+--------+------------+------------+---------+-----+---------+------------+-----------+-----------+
              |Ver|IHL |DSCP/ECN|Total Length|Identification|Flags/Frag|TTL |Protocol|Header Cksum|Source IP  |Dest IP    | Payload...
              |4b |4b  |1 byte  |2 bytes     |2 bytes       |2 bytes  |1B  |1 byte  |2 bytes     |4 bytes    |4 bytes    |
              +--------+--------+------------+------------+---------+-----+---------+------------+-----------+-----------+
```

| Byte offset | Length | Field | Meaning | Value KAOS reads or writes |
|---|---|---|---|---|
| 0, upper nibble | 4 bits | Version | The IP version this header claims to be. | Must be `4`. `parse()` returns `None` for anything else (there is no IPv6 anywhere in this codebase). |
| 0, lower nibble | 4 bits | IHL (Internet Header Length) | Header length, in 32-bit (4-byte) words — a *word* count, not a byte count. The real header length in bytes is `IHL * 4`. | Must be `>= 5`. `serialize_header()` always writes byte `0` as `0x45` (upper nibble `4`, lower nibble `5`): version 4, a 20-byte header, no options. |
| 1 | 1 byte | DSCP / ECN (historically "Type of Service") | Differentiated Services Code Point and Explicit Congestion Notification bits — quality-of-service marking and network-congestion signaling. | Always `0x00`. Neither QoS marking nor ECN is implemented; the field is present only because the header format requires the byte to exist. |
| 2–3 | 2 bytes | Total Length | The entire datagram's length in bytes, header plus payload, big-endian. | Written as `20 + payload_len`. On parse, this bounds where the payload slice ends and is checked against the actual buffer length. |
| 4–5 | 2 bytes | Identification | A per-datagram value that lets a receiver group together the fragments of one original, larger datagram. | Since this stack never fragments outgoing packets, the value only needs to be caller-supplied and opaque. `net-tools` passes the ICMP sequence number through this field purely as a debugging convenience — the protocol itself attaches no meaning to that reuse. |
| 6–7 | 2 bytes | Flags (upper 3 bits) + Fragment Offset (lower 13 bits) | See the bit-level table directly below. | Always the fixed value `0x4000`: Don't-Fragment set, no more fragments, offset zero. |
| 8 | 1 byte | Time To Live (TTL) | A hop-count budget. Every router that forwards the datagram decrements it by one; a router that would decrement it to zero discards the datagram instead (and, in a full implementation, would report that back via ICMP Time Exceeded — not implemented here, see §11). | Always `DEFAULT_TTL = 64` on output. |
| 9 | 1 byte | Protocol | Identifies which protocol is encapsulated in the payload, using IANA's IP protocol number registry. | `1` = ICMP (the only value this crate ever emits or accepts payloads for). `6` (TCP) and `17` (UDP) exist as named constants for documentation but are never produced or consumed. |
| 10–11 | 2 bytes | Header Checksum | An RFC 1071 Internet checksum computed over the header bytes *only* (never the payload). | Recomputed and validated on every `parse()`; recomputed and patched in on every `serialize_header()` — see §5.2. |
| 12–15 | 4 bytes | Source IP | The sending host's IPv4 address. | The local host's configured IP on output; read and used for ARP/reply routing on input. |
| 16–19 | 4 bytes | Destination IP | The receiving host's IPv4 address. | The target host's IP on output. On input, `parse()`'s caller (`process_ipv4()` in `stack.rs`) only continues processing if this equals the local host's own IP or the limited-broadcast address `255.255.255.255` — there is no subnet-directed broadcast handling (e.g. `192.168.1.255`), and no forwarding of packets addressed elsewhere. |
| 20–end | variable, `total_length − 20` bytes | Payload | The encapsulated message — in this codebase, always an ICMP Echo Request or Reply (§6). | — |

**Bit layout of the Flags/Fragment-Offset word** (bit numbers count down from the most significant bit of the 16-bit field, i.e. bit 15 is transmitted first):

| Bit(s) | Name | Meaning | Value KAOS uses |
|---|---|---|---|
| 15 | Reserved | Must be zero by RFC 791. | `0` |
| 14 | DF — Don't Fragment | If set, no router along the path may fragment this datagram; a router that must fragment it to forward it instead discards it. | Always `1` — this stack can neither fragment nor reassemble, so every outgoing datagram forbids fragmentation outright rather than risk a router splitting it into pieces this stack could never reassemble. |
| 13 | MF — More Fragments | If set, more fragments of this same original datagram follow. | Always `0` — this is always presented as a complete, unfragmented datagram. |
| 12–0 | Fragment Offset | The offset of this fragment's data within the original datagram, in units of 8 bytes. | Always `0` — there is only ever one "fragment": the whole datagram. |

`0x4000` in binary is `0100 0000 0000 0000`: bit 15 (Reserved) is `0`, bit 14 (DF) is `1`, bit 13 (MF) is `0`, and all 13 Fragment Offset bits are `0` — exactly "don't fragment me, this is the only piece, at offset zero".

`Ipv4Packet::parse()` requires `version == 4`, `ihl >= 5`, and — crucially — that `compute_checksum()` over the *entire header* (including the checksum field the sender computed) returns exactly zero. This is the standard self-verifying property of the Internet checksum: if you sum every 16-bit word of a correctly checksummed header, including the checksum word itself, one's-complement addition of the correct checksum against the correct data always cancels out to zero. `parse()` uses this as its sole validity check — it never separately recomputes and compares against the stored value.

### 5.2 The Checksum Algorithm, Precisely

```rust
pub fn compute_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]);
        sum = sum.wrapping_add(word as u32);
        i += 2;
    }
    if i < data.len() {
        let word = u16::from_be_bytes([data[i], 0]);   // odd trailing byte, high-order
        sum = sum.wrapping_add(word as u32);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);             // fold carry back in
    }
    !(sum as u16)                                        // one's-complement
}
```

Walk through this once with real numbers to see why it works: this is RFC 1071's "Internet checksum" — sum every 16-bit big-endian word using **one's-complement arithmetic** (accumulated here in a wider 32-bit `sum` for convenience), then fold any carry out of bit 16 back into the low 16 bits (the `while (sum >> 16) != 0` loop — a 32-bit accumulator can carry out more than once, hence a loop rather than a single fold), and finally bitwise-invert the 16-bit result. A trailing odd byte (there is none in a 20-byte IPv4 header, but there can be in an ICMP payload of odd length) is treated as the high byte of one more 16-bit word with an implicit zero low byte — never silently dropped.

This same function is reused, unchanged, by [`icmp.rs`](../lib_net/src/proto/icmp.rs) (`use super::ipv4::compute_checksum;`). That reuse is intentional and reflects the protocol reality: the Internet checksum is a generic algorithm defined once by RFC 1071 and reused by IP, ICMP, TCP, and UDP alike — it is not an IPv4-specific detail that ICMP happens to duplicate.

### 5.3 Serialization

`Ipv4Packet::serialize_header()` only ever emits the fixed, option-free 20-byte form described in the table above:

```rust
out_header[0] = 0x45;                               // version 4, IHL 5
out_header[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // flags: Don't Fragment
out_header[10..12].copy_from_slice(&[0, 0]);         // zero the checksum field first...
let csum = compute_checksum(out_header);
out_header[10..12].copy_from_slice(&csum.to_be_bytes()); // ...then fill in the real one
```

Zeroing the checksum field before computing it is not a formality — `compute_checksum` sums whatever bytes are already sitting in the checksum field, so computing over stale or garbage bytes there would produce a wrong result. There is no fragmentation and no reassembly logic anywhere in this crate — any datagram that would need to be split simply cannot be sent, and an oversized incoming fragment set could not be reassembled.

---

## 6. Layer 3 Control Traffic: ICMP Echo

[`icmp.rs`](../lib_net/src/proto/icmp.rs) implements exactly the two message types `ping` needs — nothing else. There is no Destination Unreachable, no Time Exceeded, no Redirect.

```text
byte offset  0     1     2         4            6                8
             +-----+-----+---------+------------+----------------+------------------+
             |Type |Code |Checksum |Identifier  |Sequence Number | Payload (echo)   |
             |1 B  |1 B  |2 bytes  |2 bytes     |2 bytes         | (arbitrary)      |
             +-----+-----+---------+------------+----------------+------------------+
```

| Byte offset | Length | Field | Meaning | Value KAOS reads or writes |
|---|---|---|---|---|
| 0 | 1 byte | Type | The ICMP message type. | `8` = Echo Request, `0` = Echo Reply. These are the only two values `IcmpEchoPacket` ever produces or accepts; any other type is not modeled by this crate at all. |
| 1 | 1 byte | Code | A sub-type within the message Type, used by ICMP messages that have more than one kind of the same Type (for example, Destination Unreachable has separate codes for "network unreachable" vs. "port unreachable"). | Always `0` — Echo Request and Echo Reply each have exactly one sub-type. |
| 2–3 | 2 bytes | Checksum | An RFC 1071 Internet checksum, computed over the **entire ICMP message** — this 8-byte header plus the payload that follows it — unlike IPv4's checksum, which covers only its own fixed 20-byte header. This has to be so because, unlike the IPv4 header, ICMP Echo's payload length is caller-defined and must be protected too. | Recomputed and validated on every `parse()`; recomputed and patched in (after first being zeroed, same two-pass pattern as IPv4) on every `serialize()`. |
| 4–5 | 2 bytes | Identifier | An arbitrary per-session tag chosen by the sender of a Request and echoed back unchanged in the matching Reply, letting one host tell its own ping traffic apart from anyone else's on the same link. | `net-tools` always uses the fixed value `0x1337` for every `ping` invocation. |
| 6–7 | 2 bytes | Sequence Number | Incremented by the sender for each successive Echo Request within one `ping` run, and echoed back unchanged in the Reply, so a delayed or duplicated reply can be matched to the exact request that caused it. | `net-tools` sends sequence numbers `1`, `2`, `3`, `4` — one per probe of a four-probe `ping` run. |
| 8–end | variable | Payload | Arbitrary bytes chosen by the sender and echoed back byte-for-byte, unexamined, by the responder. | KAOS always sends the fixed ASCII string `b"KAOS Ping Payload 1234567890"` (29 bytes); round-trip time is measured separately via the CPU timestamp counter, not by embedding a timestamp in this field the way some `ping` implementations do. |

`NetworkStack::process_ipv4()` handles the two message types very differently: an incoming `ECHO_REQUEST` addressed to this host is answered automatically and immediately, copying the identifier, sequence number, and payload verbatim into a new `ECHO_REPLY`, wrapping it in a fresh IPv4 header (source and destination swapped) and a fresh Ethernet header. The destination MAC for that reply is looked up in the ARP cache by the *IPv4* source address; if that lookup fails (the sender's mapping was never cached — for instance if this is the very first packet ever seen from that address), the code falls back to the Ethernet source address already present on the *incoming* frame, rather than broadcasting the reply to the entire segment. An incoming `ECHO_REPLY`, by contrast, is not acted upon inside the stack at all — it only produces a `NetworkEvent::IcmpEchoReply` carrying the source IP, identifier, sequence number, TTL, and payload length, leaving it entirely up to the caller (`net-tools`, see §9) to decide whether that reply matches a `ping` it is currently waiting on.

---

## 7. `NetworkStack`: The Coordinator

[`stack.rs`](../lib_net/src/stack.rs) is the one piece of this crate that is not a pure protocol codec — it is the glue that decides, for each incoming frame, which parser to call and whether an automatic reply is warranted. Its state is small and entirely in-memory:

```rust
pub struct NetworkStack {
    pub config: NetworkConfig,      // this host's own MAC/IP/mask/gateway/DNS
    pub arp_table: ArpTable,
    pub rx_packets: usize,
    pub tx_packets: usize,
    pub rx_bytes: usize,
    pub tx_bytes: usize,
}
```

Its single receive entry point, `handle_rx_packet()`, is deliberately generic over how a reply gets transmitted:

```rust
pub fn handle_rx_packet<F>(&mut self, packet_data: &[u8], tx_fn: F) -> NetworkEvent
where
    F: FnMut(&[u8]),
```

`tx_fn` is a closure the *caller* provides — it might be `|pkt| device.transmit(pkt)` inside a NIC driver process, talking directly to hardware, or `|pkt| client.send(pkt)` inside `net-tools`, which has no hardware access at all and instead makes a `NetSend` syscall to a driver process running elsewhere. `NetworkStack` itself never knows or cares which of those it is calling into. This dependency-inversion is what makes the exact same crate usable, unmodified, on both sides of the syscall boundary described in §9 — the entire ARP/ICMP auto-reply logic is written and tested exactly once.

`handle_rx_packet()`'s steps are: parse the Ethernet frame; drop it silently if the destination MAC is neither broadcast nor this host's own MAC (basic Layer-2 filtering, done in software here because none of the NICs in this codebase are asked to filter in hardware); then dispatch purely on EtherType to `process_arp()` or `process_ipv4()` (§4.4, §6). Any other EtherType, or any frame that fails to parse at any layer, is dropped and reported as `NetworkEvent::None` — never a panic, never an error propagated up; a corrupt or unrecognized frame from the wire is an expected occurrence, not a bug.

Two more methods exist purely to let a caller **originate** traffic rather than only auto-reply to it: `build_arp_request()` (used to resolve a next hop before the very first packet to it can be sent) and `build_ping()` (used to construct an outgoing Echo Request). Both simply chain the same `serialize()` calls described above and are the actual building blocks `net-tools`'s `ping` command is written on top of (§9).

---

## 8. `NetworkEvent` and `NetworkConfig`

[`event.rs`](../lib_net/src/event.rs) defines a small closed set of outcomes `handle_rx_packet()` can report:

```rust
pub enum NetworkEvent {
    ArpRequestAnswered { sender_ip: Ipv4Address, sender_mac: MacAddress },
    ArpReplyReceived   { sender_ip: Ipv4Address, sender_mac: MacAddress },
    IcmpEchoReply      { src_ip: Ipv4Address, identifier: u16, sequence: u16, ttl: u8, data_len: usize },
    IcmpEchoRequestAnswered { src_ip: Ipv4Address, sequence: u16 },
    None,
}
```

This is a pull-based notification, not a callback registry or an event bus — every call to `handle_rx_packet()` returns exactly one of these values, and it is entirely the caller's responsibility to inspect it (or ignore it, as the background driver loop does — see §9).

[`config.rs`](../lib_net/src/config.rs) holds the interface's static configuration:

```rust
pub fn default_qemu(mac: MacAddress) -> Self {
    Self {
        mac,
        ip:          Ipv4Address::new(192, 168, 1, 200),
        gateway:     Ipv4Address::new(192, 168, 1, 1),
        subnet_mask: Ipv4Address::new(255, 255, 255, 0),
        dns:         Ipv4Address::new(192, 168, 1, 3),
    }
}
```

Every `NetworkStack` is constructed with these defaults, matched to the bridged QEMU LAN both NIC drivers attach to (`docs/drivers.md` §13 describes the `vmnet-bridged`/`tap0` QEMU networking setup). Note that `dns` is stored and displayed by `ifconfig`, but nothing in this crate — or anywhere else in the repository — ever resolves a name against it; there is no DNS client. There is also no DHCP client: every one of these values is a compile-time constant.

---

## 9. From Hardware to Two Independent Stack Instances

`lib_net` on its own only defines behavior for a single process holding one `NetworkStack` and one `NicDevice`. The current architecture, however, splits that single conceptual role across **two separate Ring-3 processes**, each running its own copy of `NetworkStack` over the same syscall-mediated channel. Understanding this split is essential to understanding how `ping` actually works end to end.

### 9.1 The Driver Process Owns the Hardware and One `NetworkStack`

A NIC driver binary (`rtl8139.bin`/`intel_nic.bin`) discovers its device, maps its MMIO BARs, sets up DMA rings exactly as described in `docs/drivers.md`, and then constructs one `NetworkStack::new(mac)` using the hardware's own burned-in MAC address. It never returns to its own `main()` after that — instead it calls [`lib_driver_runtime::run_background_driver()`](../lib_driver_runtime/src/repl.rs), a function generic over any `NicDevice` implementation, so both `rtl8139` and `intel_nic` share the identical event loop:

```rust
loop {
    // 1) drain queued App -> Driver packets and transmit them on real hardware
    while let Ok(len) = net_recv(own_id, &mut tx_buf, 0) {
        let _ = device.transmit(&tx_buf[..len]);
    }
    // 2) poll hardware RX, run them through this driver's own NetworkStack,
    //    forward every frame to waiting apps regardless of what the stack did with it
    while let Some(len) = device.poll_next_packet(&mut rx_buf) {
        let _event = stack.handle_rx_packet(&rx_buf[..len], |tx_pkt| { let _ = device.transmit(tx_pkt); });
        let _ = net_send(own_id, &rx_buf[..len]);
    }
    // 3) publish MAC/IP/counters/ARP table for DrvQuery
    let status = build_status(&stack);
    let _ = publish_status(&status);
    // 4) cooperative yield
    yield_now();
}
```

This loop's own `NetworkStack` is what actually answers ARP requests and ICMP Echo Requests addressed to the driver's own IP — an *incoming ping from another machine* is answered entirely inside this loop, in step 2, without any application process being involved at all. Step 2's `net_send(own_id, ...)` line runs unconditionally, whatever `handle_rx_packet()` returned: every received frame is *also* forwarded, verbatim, to any application waiting on this driver's RX ring — auto-reply and forwarding are not exclusive alternatives.

The driver never registers itself under a fixed process name by accident: `drv_register("nic:rtl8139")` and the matching `drv_lookup()` are two of six new syscalls (`DrvRegister`/`DrvLookup`/`NetSend`/`NetRecv`/`DrvPublishStatus`/`DrvQuery`, numbers 39–44) that turn a driver process into an addressable, queryable service, in [`lib_driver/src/drv.rs`](../lib_driver/src/drv.rs). `NetSend`/`NetRecv` are role-based rather than simple point-to-point queues: whether a given call reads/writes the driver's TX ring or its RX ring depends on whether the calling task's own id equals the target driver's id — the driver calling `net_send(own_id, frame)` on itself lands the frame in its RX ring (Driver → App direction), while an application calling `net_send(driver_id, frame)` lands it in the driver's TX ring (App → Driver direction). This single mechanism is what lets `net-tools` and the driver exchange raw Ethernet frames across a process boundary without either one needing shared memory.

### 9.2 The Client Process (`net-tools`) Owns a Second, Independent `NetworkStack`

[`user_programs/net_tools/src/main.rs`](../user_programs/net_tools/src/main.rs) is a completely ordinary Ring-3 program — it holds no `DriverCaps`, maps no MMIO, allocates no DMA. On startup it resolves whichever NIC driver is currently loaded (`"nic:rtl8139"` or `"nic:intel_nic"`, probed in that order) via `NicClient::open()`, reads that driver's published `UserDriverStatus` via `query_status()` (a `DrvQuery` syscall), and uses it to seed a **second, independent** `NetworkStack::new(mac)` — same MAC, IP, gateway, mask copied in once at startup, but a brand-new, initially empty `ArpTable` that is never resynchronized with the driver's own table afterward. From that point on, `net-tools` talks to the network exclusively through `client.send(frame)` / `client.recv(buf, timeout_ms)`, which are thin wrappers over `NetSend`/`NetRecv` (`lib_driver/src/client.rs`) — it never learns anything about the driver's internal `ArpTable` beyond that one-time snapshot.

This is why the `arp` command in `net-tools` does *not* print `net-tools`'s own `NetworkStack::arp_table` — it prints the driver's table, freshly re-fetched via `query_status()` every time the command runs (`format_arp_table(&status)` in `user_programs/net_tools/src/util.rs`, working from the `UserDriverStatus::arp_entries` array, capped at `MAX_ARP_ENTRIES`). The two ARP tables genuinely diverge over the life of a session: the driver's table accumulates entries from *all* observed traffic on the wire (including replies to other hosts' requests), while `net-tools`'s own local table only ever grows from replies to ARP requests `net-tools` itself issued.

### 9.3 `execute_ping()`, Step by Step

`net-tools`'s `ping <ip>` command (`execute_ping()` in `user_programs/net_tools/src/main.rs`) is the most complete illustration of every piece described above working together:

1. **Routing decision.** `stack.config.ip.is_same_subnet(target_ip, stack.config.subnet_mask)` applies the subnet mask to both addresses octet by octet (`(self_ip[i] & mask[i]) != (other_ip[i] & mask[i])`) to decide whether the destination is on the local `/24` or must be reached via the default gateway. If the destination is remote and no gateway is configured (`gateway.is_zero()`), the command prints `Destination Host Unreachable` and stops immediately — there is no actual IP routing table, only this one gateway-or-direct binary choice.
2. **ARP resolution of the next hop** (the target itself if local, otherwise the gateway). `stack.arp_table.lookup()` is tried first; on a miss, `stack.build_arp_request()` constructs a broadcast ARP request (§4.2) and sends it via `client.send()`. The code then spins for up to 20 seconds, reading the CPU timestamp counter (`RDTSC`) directly rather than calling into any timer syscall, re-transmitting the same request every 2 seconds, and on every iteration calling `client.recv(&mut rx_buf, 0)` in a drain loop (non-blocking — `timeout_ms == 0` means "poll once, return `Err(Timeout)` if nothing is queued", by design a different convention from `IrqWait`'s "0 = wait forever") and feeding every received frame through `stack.handle_rx_packet()`. That call is what actually populates `net-tools`'s own `arp_table` when the corresponding reply arrives.
3. **Four ICMP Echo Requests.** For sequence numbers 1..=4, `stack.build_ping(target_ip, dest_mac, 0x1337, seq, payload, &mut buf)` constructs a complete Ethernet+IPv4+ICMP frame (§6), sent via `client.send()`. The code then polls for up to 2 seconds per sequence number, again draining `client.recv()` through `handle_rx_packet()`, but this time inspecting the returned `NetworkEvent` for exactly `IcmpEchoReply { src_ip, identifier, sequence, .. }` where `src_ip == target_ip && identifier == 0x1337 && sequence == seq` — any other event (an unrelated ARP packet, a ping reply for a different sequence number, one meant for another identifier entirely) is silently ignored, because the shared driver's RX ring can and does deliver traffic that has nothing to do with this particular `ping` invocation.
4. **RTT measurement**, again via raw `RDTSC` deltas, converted to milliseconds by assuming a fixed 2 GHz TSC frequency (`cycles / 2_000_000`) — accurate only under the matching QEMU configuration this codebase targets, not a general-purpose time source on arbitrary hardware.
5. **Summary statistics** — packets transmitted vs. received, with loss percentage computed as `((transmitted - received) * 100) / transmitted`.

Tracing a single successful `ping 192.168.1.1` therefore crosses the process/hardware boundary four times: `net-tools` (Layer 3/2 construction) → `NetSend` syscall → driver's TX ring → driver's background loop → `device.transmit()` (real MMIO/DMA) → wire → remote host → wire → driver's `device.poll_next_packet()` (real MMIO/DMA) → driver's own `NetworkStack::handle_rx_packet()` (which recognizes this as a reply, not a request, and does nothing but produce an event the driver loop discards) → unconditional `NetSend` back to the app → `net-tools`'s `client.recv()` → `net-tools`'s own, second `NetworkStack::handle_rx_packet()`, whose returned `NetworkEvent::IcmpEchoReply` is what finally prints the `64 bytes from ... time=... ms` line.

---

## 10. Testing

Every protocol module ends with the same pattern:

```rust
#[cfg(all(test, not(target_os = "none")))]
#[path = "tests/ethernet.rs"]
mod tests;
```

`lib_net` is `#![no_std]` and can be compiled either for the kernel/driver target (`x86_64-unknown-none`, where `target_os` is `"none"`) or as an ordinary host binary for `cargo test`. The `not(target_os = "none")` guard means the test modules are compiled in only for the latter — there is no QEMU, no kernel, and no hardware involved in testing a single `parse()`/`serialize()` round trip; `cargo test -p lib_net` runs entirely on the host. [`lib_net/src/proto/tests/`](../lib_net/src/proto/tests/) covers each protocol module independently (malformed lengths, checksum rejection, round-trip serialize→parse equality), and [`lib_net/src/tests/stack.rs`](../lib_net/src/tests/stack.rs) exercises `NetworkStack::handle_rx_packet()` end to end — constructing a fake incoming ARP request or ICMP Echo Request by hand and asserting on both the returned `NetworkEvent` and the bytes the stack handed to its `tx_fn` closure.

---

## 11. What This Stack Deliberately Does Not Do

This is a small, educational network stack, not a production TCP/IP implementation, and the gaps are intentional rather than accidental oversights:

- **No Layer 4 at all.** No TCP, no UDP, no port numbers, no sockets. Everything above ICMP Echo is out of scope.
- **No DHCP.** All addressing is a compile-time default (`NetworkConfig::default_qemu`).
- **No DNS resolution**, despite a configurable `dns` field existing in `NetworkConfig` purely for display.
- **No IPv4 fragmentation or reassembly.** The Don't-Fragment bit is always set on output, and there is no code path that could reassemble an incoming fragment set even if one arrived.
- **No ICMP error messages** — no Destination Unreachable, no Time Exceeded, no Redirect. Only Echo Request/Reply exist.
- **No ARP entry expiration**, only FIFO eviction once the 128-entry cache is full (§4.3), and no protection against ARP cache poisoning by a forged sender address.
- **No real IP routing** — `is_same_subnet()` plus a single configured default gateway is the entire routing decision (§9.3); there is no routing table with multiple entries.

For the hardware side of this system — PCI enumeration, MMIO register maps, DMA ring buffers, IRQ delivery, and the hardware-specific transmit/receive paths of the RTL8139 and Intel NIC drivers — see [`docs/drivers.md`](drivers.md).
