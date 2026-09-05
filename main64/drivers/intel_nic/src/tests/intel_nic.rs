use super::*;

#[test]
fn test_nic_model_names() {
    assert_eq!(
        NicModel::E1000e.name(),
        "Intel 82577LM Gigabit Network Connection"
    );
    assert_eq!(NicModel::I219V.name(), "Intel Ethernet Connection I219-V");
    assert_eq!(
        NicModel::E1000e82574L.name(),
        "Intel 82574L Gigabit Network Connection (e1000e)"
    );
    assert_eq!(
        NicModel::E100082540EM.name(),
        "Intel 82540EM Gigabit Ethernet Controller (e1000)"
    );
}

#[test]
fn test_descriptor_struct_sizes() {
    assert_eq!(core::mem::size_of::<RxDesc>(), 16);
    assert_eq!(core::mem::size_of::<TxDesc>(), 16);
}

#[test]
fn test_status_link_detection() {
    assert!(!status_has_link(0x0008_0600));
    assert!(status_has_link(0x0008_0603));
}

#[test]
fn test_tx_descriptor_control_is_model_specific() {
    let pch = e1000_tx_descriptor_control(NicModel::I219V, IGB_QUEUE_ENABLE);
    assert_eq!(pch & IGB_QUEUE_ENABLE, 0);
    assert_eq!(pch & TXDCTL_FULL_TX_DESC_WB, TXDCTL_FULL_TX_DESC_WB);
    assert_ne!(pch & TXDCTL_COUNT_DESC, 0);
    assert_eq!(pch & TXDCTL_PTHRESH, 31);
    assert_eq!(pch, 0x0141_001F);

    let standalone = e1000_tx_descriptor_control(NicModel::E1000e82574L, IGB_QUEUE_ENABLE);
    assert_eq!(standalone & IGB_QUEUE_ENABLE, 0);
    assert_eq!(standalone & TXDCTL_FULL_TX_DESC_WB, TXDCTL_FULL_TX_DESC_WB);
    assert_eq!(standalone & TXDCTL_COUNT_DESC, 0);
}

#[test]
fn test_transmit_control_preserves_82577_reset_fields() {
    let reserved_bit_28 = 1 << 28;
    let rrthresh = 1 << 29;
    let cold_reset = 0x3F << TCTL_COLD_SHIFT;
    let reset_value = reserved_bit_28 | rrthresh | cold_reset | TCTL_PSP;

    let configured = e1000_transmit_control(reset_value);

    assert_eq!(configured & reserved_bit_28, reserved_bit_28);
    assert_eq!(configured & (0b11 << 29), rrthresh);
    assert_eq!(configured & (0x3FF << TCTL_COLD_SHIFT), cold_reset);
    assert_eq!(configured & TCTL_CT_MASK, 0x0F << TCTL_CT_SHIFT);
    assert_ne!(configured & TCTL_EN, 0);
    assert_ne!(configured & TCTL_RTLC, 0);
}

#[test]
fn test_transmit_ipg_is_model_specific() {
    assert_eq!(e1000_transmit_ipg(NicModel::E1000e), 0x0070_2008);
    assert_eq!(e1000_transmit_ipg(NicModel::I219V), 0x0070_2008);
    assert_eq!(e1000_transmit_ipg(NicModel::E1000e82574L), 0x0060_200A);
    assert_eq!(e1000_transmit_ipg(NicModel::E100082540EM), 0x0060_200A);
}

#[test]
fn test_pch_reset_control_honors_firmware_phy_permission() {
    let base = CTRL_GIO_MASTER_DISABLE;

    let permitted = reset_control(NicModel::E1000e, base, FWSM_RSPCIPHY);
    assert_ne!(permitted & CTRL_RST, 0);
    assert_ne!(permitted & CTRL_PHY_RST, 0);
    assert_ne!(permitted & CTRL_GIO_MASTER_DISABLE, 0);

    let blocked = reset_control(NicModel::E1000e, base, 0);
    assert_ne!(blocked & CTRL_RST, 0);
    assert_eq!(blocked & CTRL_PHY_RST, 0);

    let standalone = reset_control(NicModel::E1000e82574L, 0, FWSM_RSPCIPHY);
    assert_ne!(standalone & CTRL_RST, 0);
    assert_eq!(standalone & CTRL_PHY_RST, 0);
}

#[test]
fn test_ich_pch_transmit_arbitration_bits() {
    assert_eq!(ich_tarc0(0) & ICH_TARC0_REQUIRED, ICH_TARC0_REQUIRED);

    let single_request = ich_tarc1(0, 0);
    assert_eq!(single_request & ICH_TARC1_REQUIRED, ICH_TARC1_REQUIRED);
    assert_ne!(single_request & (1 << 28), 0);

    let multiple_request = ich_tarc1(u32::MAX, TCTL_MULR);
    assert_eq!(multiple_request & (1 << 28), 0);
}

#[test]
fn test_model_specific_mta_register_counts() {
    assert_eq!(NicModel::E1000e.mta_register_count(), 32);
    assert_eq!(NicModel::I219V.mta_register_count(), 32);
    assert_eq!(NicModel::E1000e82574L.mta_register_count(), 128);
    assert_eq!(NicModel::E100082540EM.mta_register_count(), 128);
}

#[test]
fn test_ich_pch_packet_buffer_allocation() {
    assert_eq!(
        NicModel::E1000e.packet_buffer_allocation_rx_kb(),
        Some(ICH_PCH_PBA_RX_KB)
    );
    assert_eq!(
        NicModel::I219V.packet_buffer_allocation_rx_kb(),
        Some(ICH_PCH_PBA_RX_KB)
    );
    assert_eq!(
        NicModel::E1000e82574L.packet_buffer_allocation_rx_kb(),
        None
    );
    assert_eq!(
        NicModel::E100082540EM.packet_buffer_allocation_rx_kb(),
        None
    );
}

#[test]
fn test_prepare_tx_frame_zero_pads_short_packets() {
    let packet = [0xA5; 42];
    let mut tx_slot = [0xCC; 128];

    let length = prepare_tx_frame(&packet, &mut tx_slot).expect("prepare frame");

    assert_eq!(length, 60);
    assert_eq!(&tx_slot[..packet.len()], &packet);
    assert!(tx_slot[packet.len()..length].iter().all(|byte| *byte == 0));
    assert!(tx_slot[length..].iter().all(|byte| *byte == 0xCC));
}

#[test]
fn test_received_frame_requires_eop_and_rejects_frame_errors() {
    let complete = RX_STATUS_DD | RX_STATUS_EOP;

    assert_eq!(received_frame_len(complete, 0, 64, 128), Some(64));
    assert_eq!(received_frame_len(RX_STATUS_DD, 0, 64, 128), None);
    assert_eq!(received_frame_len(complete, 0x80, 64, 128), None);
    assert_eq!(received_frame_len(complete, 0, 129, 128), None);
}

#[test]
fn test_multi_descriptor_frame_continuation_is_dropped_not_desynchronized() {
    let complete = RX_STATUS_DD | RX_STATUS_EOP;
    let fragment_start = RX_STATUS_DD; // DD set, EOP not set: first descriptor of an oversized frame.

    // An ordinary single-descriptor frame is unaffected.
    let (len, mid) = received_frame_len_multi(false, complete, 0, 64, 128);
    assert_eq!(len, Some(64));
    assert!(!mid);

    // First descriptor of a frame spanning more than one buffer: dropped,
    // and the mid-frame state latches on so the next descriptor is not
    // mistaken for an independent frame.
    let (len, mid) = received_frame_len_multi(false, fragment_start, 0, 64, 128);
    assert_eq!(len, None);
    assert!(mid);

    // The EOP-bearing descriptor that closes the frame out must also be
    // dropped — even though it is itself well-formed and complete — because
    // treating it as an independent frame would desynchronize reassembly.
    let (len, mid) = received_frame_len_multi(true, complete, 0, 64, 128);
    assert_eq!(
        len, None,
        "a continuation fragment must never be reported as its own frame"
    );
    assert!(
        !mid,
        "EOP on the continuation must clear the mid-frame state"
    );
}
