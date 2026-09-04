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
