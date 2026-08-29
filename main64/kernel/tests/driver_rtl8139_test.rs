//! Integration tests for the RTL8139 user-space driver lifecycle and abstractions.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec;
use core::panic::PanicInfo;

use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::process::capabilities::{Capabilities, DriverCaps, ResourceGrants};
use kaos_kernel::scheduler::{
    self as sched, set_running_slot_for_test, set_task_caps, task_id_slot,
};
use kaos_kernel::syscall::{dispatch_checked, SyscallId, SYSCALL_OK};

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);
    sched::init();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

extern "C" fn test_task_loop() -> ! {
    loop {
        sched::yield_now();
    }
}

/// Tests that a simulated RTL8139 driver task with MMIO and IRQ capabilities
/// can map its device registers and subscribe to its interrupt line.
#[test_case]
fn test_rtl8139_driver_grants_and_mapping() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    // Simulated RTL8139 BAR physical 0xFEB0_0000, 256 bytes, IRQ line 11
    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::MMIO | Capabilities::IRQ,
        grants,
    )));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Map RTL8139 MMIO BAR
    let va = dispatch_checked(SyscallId::MAP_PHYSICAL, 0xFEB0_0000, 256, 0, 0)
        .expect("RTL8139 MMIO mapping must succeed");
    assert_eq!(
        va,
        vmm::USER_MMIO_BASE,
        "MMIO mapping starts at USER_MMIO_BASE"
    );

    // Subscribe to RTL8139 IRQ
    let irq_res = dispatch_checked(SyscallId::IRQ_SUBSCRIBE, 11, 0, 0, 0);
    assert_eq!(
        irq_res,
        Ok(SYSCALL_OK),
        "RTL8139 IRQ subscription must succeed"
    );

    // Clean up
    let unmap_res = dispatch_checked(SyscallId::UNMAP_PHYSICAL, va, 256, 0, 0);
    assert_eq!(unmap_res, Ok(SYSCALL_OK), "Unmap MMIO must succeed");

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
    kaos_kernel::drivers::irq_bridge::reset_bindings_for_test();
}

/// Tests that a driver task with MMIO capabilities can allocate contiguous DMA buffers,
/// translate their virtual addresses to physical frames, and free them cleanly.
#[test_case]
fn test_rtl8139_driver_dma_allocation_and_translation() {
    let task_id = sched::spawn_kernel_task(test_task_loop).expect("spawn task");
    let slot = task_id_slot(task_id);

    let grants = ResourceGrants {
        mmio_regions: vec![(0xFEB0_0000, 256)],
        irqs: vec![11],
        mmio_bump: vmm::USER_MMIO_BASE,
    };
    let caps_ptr = Box::into_raw(Box::new(DriverCaps::new(
        Capabilities::MMIO | Capabilities::IRQ,
        grants,
    )));
    set_task_caps(task_id, caps_ptr);
    set_running_slot_for_test(Some(slot));

    // Step 1: Map a user page for the out_phys pointer parameter.
    let user_out_va = vmm::USER_HEAP_BASE + 0x4000;
    let out_phys_frame = vmm::page_table::alloc_frame_phys().unwrap();
    let out_pfn = vmm::page_table::phys_to_pfn(out_phys_frame);
    vmm::map_user_page(user_out_va, out_pfn, true).unwrap();

    // Step 2: Allocate a 4-page (16 KiB) contiguous DMA buffer.
    let va = dispatch_checked(SyscallId::ALLOC_DMA, 4, user_out_va, 0, 0)
        .expect("AllocDma must succeed");
    assert!(va >= vmm::USER_MMIO_BASE);

    // SAFETY: user_out_va is mapped and initialized by AllocDma.
    let out_phys = unsafe { core::ptr::read(user_out_va as *const u64) };
    assert_ne!(
        out_phys, 0,
        "AllocDma must return non-zero physical address"
    );

    // Step 3: Translate virtual address back to physical address.
    let translated_phys =
        dispatch_checked(SyscallId::VIRT_TO_PHYS, va, 0, 0, 0).expect("VirtToPhys must succeed");
    assert_eq!(
        translated_phys, out_phys,
        "VirtToPhys must match physical address returned by AllocDma"
    );

    // Step 4: Free the DMA buffer.
    let free_res = dispatch_checked(SyscallId::FREE_DMA, va, 4, 0, 0);
    assert_eq!(free_res, Ok(SYSCALL_OK), "FreeDma must succeed");

    vmm::unmap_without_release(user_out_va);
    pmm::with_pmm(|mgr| mgr.release_pfn(out_pfn));

    set_running_slot_for_test(None);
    sched::terminate_task(task_id);
}

#[allow(dead_code, unused_imports)]
#[path = "../../user_programs/rtl8139/src/net/mod.rs"]
mod net;

use net::{
    arp_opcode, ethertype, icmp_type, ip_protocol, ArpPacket, ArpTable, EthernetFrame,
    IcmpEchoPacket, Ipv4Address, Ipv4Packet, MacAddress, NetworkEvent, NetworkStack,
};

/// Tests Ethernet frame serialization and deserialization in kernel test environment.
#[test_case]
fn test_rtl8139_ethernet_frame_serialization_and_parsing() {
    let src = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let dest = MacAddress::BROADCAST;
    let payload = b"KAOS Ethernet Frame Integration Test";

    let frame = EthernetFrame {
        dest_mac: dest,
        src_mac: src,
        ethertype: ethertype::IPV4,
        payload,
    };

    let mut buffer = [0u8; 128];
    let written = frame.serialize(&mut buffer).expect("serialize frame");
    assert_eq!(written, 14 + payload.len());

    let parsed = EthernetFrame::parse(&buffer[..written]).expect("parse frame");
    assert_eq!(parsed.dest_mac, dest);
    assert_eq!(parsed.src_mac, src);
    assert_eq!(parsed.ethertype, ethertype::IPV4);
    assert_eq!(parsed.payload, payload);
}

/// Tests ARP packet construction, parsing, and ARP table caching.
#[test_case]
fn test_rtl8139_arp_packet_generation_and_caching() {
    let sender_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let sender_ip = Ipv4Address::new(10, 0, 2, 15);
    let target_ip = Ipv4Address::new(10, 0, 2, 2);

    // Step 1: Build and parse ARP request.
    let req = ArpPacket::build_request(sender_mac, sender_ip, target_ip);
    assert_eq!(req.opcode, arp_opcode::REQUEST);

    let mut buf = [0u8; 64];
    let written = req.serialize(&mut buf).expect("serialize ARP request");
    assert_eq!(written, 28);

    let parsed = ArpPacket::parse(&buf[..written]).expect("parse ARP request");
    assert_eq!(parsed.sender_mac, sender_mac);
    assert_eq!(parsed.sender_ip, sender_ip);
    assert_eq!(parsed.target_ip, target_ip);

    // Step 2: Test ARP table resolution and updates.
    let mut table = ArpTable::new();
    let gateway_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    assert_eq!(table.lookup(target_ip), None);
    table.update(target_ip, gateway_mac);
    assert_eq!(table.lookup(target_ip), Some(gateway_mac));
}

/// Tests IPv4 header checksum calculation and packet validation.
#[test_case]
fn test_rtl8139_ipv4_checksum_and_validation() {
    let src = Ipv4Address::new(10, 0, 2, 15);
    let dest = Ipv4Address::new(10, 0, 2, 2);
    let payload = b"KAOS IPv4 Checksum Payload";

    let mut packet_buf = [0u8; 128];
    let mut header = [0u8; 20];
    Ipv4Packet::serialize_header(
        src,
        dest,
        ip_protocol::ICMP,
        payload.len(),
        0x4321,
        64,
        &mut header,
    );

    packet_buf[0..20].copy_from_slice(&header);
    packet_buf[20..20 + payload.len()].copy_from_slice(payload);

    let total_len = 20 + payload.len();
    let parsed = Ipv4Packet::parse(&packet_buf[..total_len]).expect("parse valid IPv4 packet");

    assert_eq!(parsed.version, 4);
    assert_eq!(parsed.src_ip, src);
    assert_eq!(parsed.dest_ip, dest);
    assert_eq!(parsed.protocol, ip_protocol::ICMP);
    assert_eq!(parsed.payload, payload);

    // Test subnet matching
    let mask = Ipv4Address::new(255, 255, 255, 0);
    let local_ip = Ipv4Address::new(192, 168, 1, 100);
    let gateway_ip = Ipv4Address::new(192, 168, 1, 1);
    let remote_ip = Ipv4Address::new(8, 8, 8, 8);
    assert!(local_ip.is_same_subnet(gateway_ip, mask));
    assert!(!local_ip.is_same_subnet(remote_ip, mask));
}

/// Tests ICMP Echo Request and Reply packet construction and checksum validation.
#[test_case]
fn test_rtl8139_icmp_echo_request_and_reply() {
    let payload = b"KAOS ICMP Ping Echo Test Payload";
    let req = IcmpEchoPacket::build_echo_request(0xBEEF, 42, payload);

    let mut buffer = [0u8; 128];
    let written = req.serialize(&mut buffer).expect("serialize ICMP request");

    let parsed = IcmpEchoPacket::parse(&buffer[..written]).expect("parse ICMP request");
    assert_eq!(parsed.icmp_type, icmp_type::ECHO_REQUEST);
    assert_eq!(parsed.identifier, 0xBEEF);
    assert_eq!(parsed.sequence_number, 42);
    assert_eq!(parsed.payload, payload);

    let reply =
        IcmpEchoPacket::build_echo_reply(parsed.identifier, parsed.sequence_number, parsed.payload);
    let mut reply_buf = [0u8; 128];
    let reply_len = reply
        .serialize(&mut reply_buf)
        .expect("serialize ICMP reply");

    let parsed_reply = IcmpEchoPacket::parse(&reply_buf[..reply_len]).expect("parse ICMP reply");
    assert_eq!(parsed_reply.icmp_type, icmp_type::ECHO_REPLY);
    assert_eq!(parsed_reply.identifier, 0xBEEF);
    assert_eq!(parsed_reply.sequence_number, 42);
}

/// Tests NetworkStack coordinator auto-response to incoming ARP and ICMP requests.
#[test_case]
fn test_rtl8139_network_stack_auto_response() {
    let my_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    let mut stack = NetworkStack::new(my_mac);

    let gateway_mac = MacAddress::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    let gateway_ip = Ipv4Address::new(10, 0, 2, 2);

    // Simulate incoming ARP Request from Gateway asking for our IP
    let arp_req = ArpPacket::build_request(gateway_mac, gateway_ip, stack.config.ip);
    let mut arp_buf = [0u8; 28];
    let arp_len = arp_req.serialize(&mut arp_buf).unwrap();

    let eth_req = EthernetFrame {
        dest_mac: MacAddress::BROADCAST,
        src_mac: gateway_mac,
        ethertype: ethertype::ARP,
        payload: &arp_buf[..arp_len],
    };
    let mut frame_buf = [0u8; 64];
    let frame_len = eth_req.serialize(&mut frame_buf).unwrap();

    let mut sent_packets = alloc::vec::Vec::new();
    let event = stack.handle_rx_packet(&frame_buf[..frame_len], |tx_pkt| {
        sent_packets.push(alloc::vec::Vec::from(tx_pkt));
    });

    assert_eq!(
        event,
        NetworkEvent::ArpRequestAnswered {
            sender_ip: gateway_ip,
            sender_mac: gateway_mac,
        }
    );
    assert_eq!(
        sent_packets.len(),
        1,
        "Stack must auto-reply to ARP request for its IP"
    );
    assert_eq!(stack.arp_table.lookup(gateway_ip), Some(gateway_mac));
}
