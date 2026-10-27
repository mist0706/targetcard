//! Does the parser actually learn a player's server from real traffic?
//!
//! The card resolves a shared name by asking the API about the server the game
//! stated for that player. That only works if the parser reads it, and the first
//! version of the read looked past the end of a frame in a way a hand-written
//! fixture would never show. This replays a capture through the same path the
//! running app uses.
//!
//! `TC_CAPTURE=<path to packets_*.txt>`; skips when unset.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use a2tools_dps_meter_lib::capture::packet_accumulator::PacketAccumulator;
use a2tools_dps_meter_lib::capture::stream_processor::StreamProcessor;
use a2tools_dps_meter_lib::combat::data_storage::DataStorage;
use a2tools_dps_meter_lib::i18n::lookup::{NpcLookup, SkillLookup};
use a2tools_dps_meter_lib::share::read_capture;

#[test]
fn a_real_capture_yields_the_servers_the_game_stated() {
    let Ok(path) = std::env::var("TC_CAPTURE") else {
        return;
    };
    let packets = read_capture(Path::new(&path)).expect("capture");
    let storage = Arc::new(DataStorage::new());
    let mut processor = StreamProcessor::new(
        storage.clone(),
        Arc::new(SkillLookup::new()),
        Arc::new(NpcLookup::new()),
    );

    let mut streams: HashMap<String, PacketAccumulator> = HashMap::new();
    for packet in &packets {
        let accumulator = streams
            .entry(packet.stream.clone())
            .or_insert_with(PacketAccumulator::new);
        accumulator.append(&packet.bytes);
        let used = processor.consume_stream(accumulator.snapshot());
        if used > 0 {
            accumulator.discard_bytes(used);
        }
    }

    // Ground truth from the game's own match history: AJIXuMuK is on 2316 (Kromede),
    // TankiNator on 2308 (Beritra), and the local player on 1313 (Phernos).
    for name in ["AJIXuMuK", "TankiNator", "Kaze", "Sinqy", "Sins"] {
        println!("{name} -> {}", storage.player_server(name));
    }
    assert_eq!(storage.player_server("Sins"), 1313, "the local player's own record");
    let ajix = storage.player_server("AJIXuMuK");
    assert!(
        ajix == 0 || ajix == 2316,
        "AJIXuMuK, when this capture contains them, is on 2316, not {ajix}"
    );
}
