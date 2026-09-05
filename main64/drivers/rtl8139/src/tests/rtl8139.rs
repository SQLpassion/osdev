use super::*;

#[test]
fn test_prepare_tx_frame_zero_pads_short_packets() {
    let packet = [0xA5; 42];
    let mut tx_slot = [0xCC; 128];

    let length = prepare_tx_frame(&packet, &mut tx_slot).expect("prepare frame");

    assert_eq!(length, 60);
    assert_eq!(&tx_slot[..packet.len()], &packet);
    assert!(
        tx_slot[packet.len()..length].iter().all(|byte| *byte == 0),
        "padding bytes up to the 60-byte minimum must be zeroed, not leaked from a previous slot use"
    );
    assert!(tx_slot[length..].iter().all(|byte| *byte == 0xCC));
}

#[test]
fn test_prepare_tx_frame_reuses_stale_slot_without_leaking() {
    // Simulate a TX slot still holding a previous, longer packet's bytes.
    let mut tx_slot = [0xEE; 128];

    let short_packet = [0x11; 10];
    let length = prepare_tx_frame(&short_packet, &mut tx_slot).expect("prepare frame");

    assert_eq!(length, 60);
    assert_eq!(&tx_slot[..short_packet.len()], &short_packet);
    assert!(
        tx_slot[short_packet.len()..length]
            .iter()
            .all(|byte| *byte == 0),
        "stale bytes from the slot's previous packet must not be retransmitted as padding"
    );
}

#[test]
fn test_prepare_tx_frame_passes_through_full_length_packets() {
    let packet = [0x42; 100];
    let mut tx_slot = [0u8; 128];

    let length = prepare_tx_frame(&packet, &mut tx_slot).expect("prepare frame");

    assert_eq!(length, packet.len());
    assert_eq!(&tx_slot[..packet.len()], &packet);
}

#[test]
fn test_prepare_tx_frame_rejects_empty_or_oversized_packets() {
    let mut tx_slot = [0u8; 128];

    assert!(prepare_tx_frame(&[], &mut tx_slot).is_none());

    let oversized = [0u8; 129];
    assert!(prepare_tx_frame(&oversized, &mut tx_slot).is_none());
}

#[test]
fn test_rx_frame_is_ok_accepts_a_clean_frame() {
    assert!(
        rx_frame_is_ok(RX_STATUS_ROK, 64),
        "ROK set, no error bits, in-bounds length must be accepted"
    );
}

#[test]
fn test_rx_frame_is_ok_rejects_error_bits_even_with_rok_set() {
    // Real RTL8139 hardware can latch ROK together with a corruption bit on
    // a bad frame; the driver must not trust ROK alone.
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK | RX_STATUS_CRC, 64),
        "a CRC error alongside ROK must reject the frame"
    );
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK | RX_STATUS_FAE, 64),
        "a frame alignment error alongside ROK must reject the frame"
    );
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK | RX_STATUS_LONG, 64),
        "a long-packet error alongside ROK must reject the frame"
    );
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK | RX_STATUS_RUNT, 64),
        "a runt-packet error alongside ROK must reject the frame"
    );
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK | RX_STATUS_ISE, 64),
        "an invalid-symbol error alongside ROK must reject the frame"
    );
}

#[test]
fn test_rx_frame_is_ok_rejects_missing_rok_or_bad_length() {
    assert!(
        !rx_frame_is_ok(0, 64),
        "a frame with ROK unset must be rejected regardless of length"
    );
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK, 3),
        "a length below the 4-byte CRC-only minimum must be rejected"
    );
    assert!(
        !rx_frame_is_ok(RX_STATUS_ROK, 1793),
        "a length above the 1792-byte maximum must be rejected"
    );
}
